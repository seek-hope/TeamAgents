------------------------------ MODULE V2Images -------------------------------
(***************************************************************************)
(* The image flow (D-392): a `view_image` result is a *reference* at rest, and   *)
(* the request builder either loads it into a real image part or substitutes    *)
(* the placeholder text. Two claims matter, and both are about a wire that      *)
(* must never carry something the model or the provider cannot take:            *)
(*                                                                          *)
(*   NoImagesWithoutSupport — a request for a model that did not declare        *)
(*     `images = true` never carries an image part. The capability is declared  *)
(*     and fail-closed, so a model that was never checked is sent the           *)
(*     placeholder instead of a block its provider would reject or drop.        *)
(*   PartsOnlyFromLoadedBytes — an image part exists only for a reference whose *)
(*     bytes were really read; an unreadable, oversized or wrong-typed file     *)
(*     becomes the error text in the same place, never a block.                 *)
(*                                                                          *)
(* Code anchors: engine/src/tools.rs — expand_image_reference (the build step   *)
(* and the two rules), load_image_reference (the loader, which re-validates the *)
(* root and the recorded media type), read_image (the tool's own size, format   *)
(* and type checks at dispatch); engine/src/v2/driver.rs — expand_images (called *)
(* on the in-memory view before every prepare_request, including after a        *)
(* compaction), so storage keeps the reference and only the request carries     *)
(* bytes; core/src/kernel/types.rs — estimated_tokens, whose image allowance is *)
(* what keeps a base64 body from being priced by its length.                    *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS Vision,          \* did the model profile declare `images = true`?
          BlindSendsParts  \* counterfactual (D-392): expand without consulting the capability

ASSUME Vision \in BOOLEAN

RefState == {"none", "present", "unreadable", "oversized", "retyped"}
Wire == {"none", "parts", "placeholder", "error_text", "text"}

VARIABLES
  ref,      \* what the context holds for the tool message
  loaded,   \* were the bytes really read?
  wire,     \* what the message carries when the request is built
  sent,     \* monitor: a request was built
  leaked,   \* monitor: a part reached a model that cannot read one
  unbacked  \* monitor: a part reached the wire without loaded bytes

vars == <<ref, loaded, wire>>
monVars == <<vars, sent, leaked, unbacked>>

\* --------------------------------------------------------------- actions --
\* `view_image` returned a reference (or refused before returning one).
StoreReference(r) ==
  /\ ~sent
  /\ ref = "none"
  /\ r \in {"present", "unreadable", "oversized", "retyped"}
  /\ ref' = r
  /\ loaded' = FALSE
  /\ wire' = "none"
  /\ UNCHANGED <<sent, leaked, unbacked>>

\* The build step asks the loader for the bytes: only a `present` reference loads.
Load ==
  /\ ~sent
  /\ ref \in {"present", "unreadable", "oversized", "retyped"}
  /\ ~loaded
  /\ loaded' = (ref = "present")
  /\ UNCHANGED <<ref, wire, sent, leaked, unbacked>>

\* ... and turns it into the wire content. This is `expand_image_reference`, and the
\* `Vision \/ BlindSendsParts` guard is exactly the rule the control removes.
Expand ==
  /\ ~sent
  /\ ref # "none"
  /\ wire = "none"
  /\ IF loaded
       THEN IF Vision \/ BlindSendsParts THEN wire' = "parts" ELSE wire' = "placeholder"
       ELSE IF Vision \/ BlindSendsParts THEN wire' = "error_text" ELSE wire' = "placeholder"
  /\ leaked' = (leaked \/ (wire' = "parts" /\ ~Vision))
  /\ unbacked' = (unbacked \/ (wire' = "parts" /\ ~loaded))
  /\ UNCHANGED <<ref, loaded, sent>>

\* A request with no image reference at all keeps its plain text content.
PlainMessage ==
  /\ ~sent
  /\ ref = "none"
  /\ wire = "none"
  /\ wire' = "text"
  /\ UNCHANGED <<ref, loaded, sent, leaked, unbacked>>

\* The request is built and leaves for the provider. This ends the message's life:
\* a sent request is never re-shaped, which is what makes the two wire claims statements
\* about a request rather than about a collection of possible ones.
Send ==
  /\ wire # "none"
  /\ ~sent
  /\ sent' = TRUE
  /\ UNCHANGED <<ref, loaded, wire, leaked, unbacked>>

Stutter == UNCHANGED monVars

Next ==
  \/ \E r \in {"present", "unreadable", "oversized", "retyped"} : StoreReference(r)
  \/ Load
  \/ Expand
  \/ PlainMessage
  \/ Send
  \/ Stutter

Init ==
  /\ ref = "none"
  /\ loaded = FALSE
  /\ wire = "none"
  /\ sent = FALSE
  /\ leaked = FALSE
  /\ unbacked = FALSE

Spec == Init /\ [][Next]_monVars

\* ------------------------------------------------------------- invariants --
TypeOK ==
  /\ ref \in {"none", "present", "unreadable", "oversized", "retyped"}
  /\ loaded \in BOOLEAN
  /\ wire \in Wire
  /\ sent \in BOOLEAN

\* D-392: a model that did not declare image support never receives an image part.
NoImagesWithoutSupport == wire = "parts" => Vision

\* ... and a part is only ever built from bytes that were really read.
PartsOnlyFromLoadedBytes == wire = "parts" => loaded

\* Monitors: a leak (a part to a blind model) and an unbacked part (a part without bytes).
NoLeakedImages == leaked = FALSE
NoUnbackedParts == unbacked = FALSE

\* A reference whose bytes cannot be read is reported as text, never dropped silently:
\* the model learns that a picture was looked at, which is what the placeholder and the
\* error line both exist for.
UnreadableIsReported ==
  ref \in {"unreadable", "oversized", "retyped"} /\ sent =>
    wire \in {"error_text", "placeholder"}

=============================================================================
