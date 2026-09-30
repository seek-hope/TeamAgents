------------------------------ MODULE V2Isolation ----------------------------
(***************************************************************************)
(* The sandbox backend choice and its two rules (D-369).                   *)
(*                                                                          *)
(* A session runs `approved_scope` commands under one configured backend    *)
(* (bubblewrap or docker). The rule the product promises is *fail closed*:  *)
(* a command runs under the configured backend or it does not run — never   *)
(* silently on another backend, and never on a backend that is not usable   *)
(* here. A14 is the bubblewrap instance of this; D-369 generalises it.      *)
(*                                                                          *)
(*   - `RunsOnlyUnderTheConfiguredBackend`: no command runs under a backend  *)
(*     other than the configured one (which is what would make               *)
(*     `sandbox = "docker"` quietly a host shell when docker is missing);    *)
(*   - `UnavailableBackendNeverRuns`: a command runs only under a backend    *)
(*     the probe accepted.                                                   *)
(*                                                                          *)
(* The two rules are independent: a fallback can be to an *available*       *)
(* backend, and an unavailable backend can be the *configured* one. The two *)
(* negative controls at the end forget one rule each and must be refuted.   *)
(* Safety only: a command need not ever run. The probe and the argv are the *)
(* code's, pinned by `tools::tests::{docker_argv_is_stable_and_runs_isolated, *)
(* a_docker_backend_without_its_image_refuses_instead_of_running_on_the_host}` *)
(* and A14's existing refusal tests.                                        *)
(*                                                                          *)
(* Code anchors: engine/src/tools.rs (`sandbox_state_for`, `shell_command_   *)
(* spec`, `docker_argv`) and engine/src/config.rs (`sandbox_from_config`).  *)
(***************************************************************************)
EXTENDS FiniteSets

CONSTANTS Backends,          \* {"bubblewrap", "docker", "host"}
          Configured,        \* the backend the user's config selected
          Available,         \* the backends the probe accepted here
          AllowFallback,     \* negative control: a command may run under another backend
          AllowUnavailable   \* negative control: a command may run under an unusable backend

ASSUME Configured \in Backends /\ Available \subseteq Backends

VARIABLES
  runs,        \* the backends commands actually ran under
  fallback,    \* monitor: a command ran under a backend other than the configured one
  unavailable  \* monitor: a command ran under a backend the probe did not accept

vars == <<runs, fallback, unavailable>>

\* ------------------------------------------------------------------ actions --
Run(b) ==
  /\ b \in Backends
  /\ (AllowFallback \/ b = Configured)
  /\ (AllowUnavailable \/ b \in Available)
  /\ runs' = runs \union {b}
  /\ fallback' = (fallback \/ (b # Configured))
  /\ unavailable' = (unavailable \/ ~(b \in Available))

Stutter == UNCHANGED vars

Next == ( \E b \in Backends : Run(b) ) \/ Stutter

Init ==
  /\ runs = {}
  /\ fallback = FALSE
  /\ unavailable = FALSE

Spec == Init /\ [][Next]_vars

\* -------------------------------------------------------------- invariants --
TypeOK ==
  /\ runs \subseteq Backends
  /\ fallback \in BOOLEAN
  /\ unavailable \in BOOLEAN

\* The rule: a command never runs on a backend other than the configured one.
RunsOnlyUnderTheConfiguredBackend ==
  fallback = FALSE

\* The rule: a command never runs on a backend the probe refused.
UnavailableBackendNeverRuns ==
  unavailable = FALSE

=============================================================================
