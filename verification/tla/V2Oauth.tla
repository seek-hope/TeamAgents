------------------------------ MODULE V2Oauth -------------------------------
(***************************************************************************)
(* MCP OAuth sign-in (D-412): the authorization-code flow with PKCE that a    *)
(* remote MCP server with no pasteable token needs.                           *)
(*                                                                           *)
(* Code anchor: engine/src/mcp_auth.rs — discover (protected-resource          *)
(* metadata, then the authorization server's own), register (RFC 7591),        *)
(* authorize (the S256 challenge and the RFC 8707 `resource`), the loopback    *)
(* callback or a pasted redirect, complete (the token exchange, which repeats  *)
(* the resource), Store (mcp-auth.json, mode 0600) and Store::access_token     *)
(* (refresh when the server issued a refresh token, refuse otherwise).         *)
(*                                                                           *)
(* Only what the five claims need is modelled: the credential one service      *)
(* holds, which resource it is bound to, where the PKCE verifier may travel,   *)
(* code lifetime and token lifetime. The environment (a clock that expires     *)
(* tokens, a user who signs out, a server that may not issue a refresh token)  *)
(* is nondeterministic on purpose.                                             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Services,       \* MCP services the user may sign in to
          Resources,      \* MCP endpoints; a token is bound to exactly one
          Codes,          \* authorization codes
          Tokens,         \* credentials the authorization server can issue
          Refreshable,    \* services whose stored credential carries a refresh token
          MaxLife,        \* how long a freshly exchanged or refreshed token lives
          MaxClock,       \* bound on the clock (keeps the state graph finite)
          IgnoreResource, \* counterfactual: present a token to another endpoint
          ReuseCode,      \* counterfactual: exchange one authorization code twice
          LeakVerifier,   \* counterfactual: send the PKCE verifier to /authorize
          IgnoreExpiry,   \* counterfactual: keep presenting an expired token
          IgnoreLogout    \* counterfactual: present a credential after it was deleted

ASSUME Services # {} /\ Resources # {} /\ Codes # {} /\ Tokens # {} /\ MaxLife > 0 /\ MaxClock > MaxLife

\* "signed out", in the same shape as a credential: TLC refuses to compare a bare value with a tuple, and modelling
\* the empty case as a credential-shaped sentinel keeps `store` a function into one set.
SignedOut == <<"none", "none">>

VARIABLES
  clock,               \* the monotone clock; expiry is read against it
  store,               \* service -> SignedOut | <<token, resource>> (what mcp-auth.json holds)
  issued,              \* the tokens the authorization server recognizes
  bound,               \* token -> "none" | resource: the endpoint it was requested for
  expiry,              \* token -> the time it stops being valid
  authorized,          \* code -> "none" | service: what /authorize issued the code for
  asked_resource,      \* code -> "none" | resource: the resource that request named
  exchanges,           \* code -> how many times it was exchanged
  verifierAtAuthorize, \* monitor: the PKCE verifier reached the authorization endpoint
  sent                 \* monitor: <<token, resource, at, service>> presented to an endpoint

vars == <<clock, store, issued, bound, expiry, authorized, asked_resource, exchanges>>
monVars == <<vars, verifierAtAuthorize, sent>>

\* ----------------------------------------------------------------- actions --
\* time passes: this is what expires a token
Advance ==
  /\ clock < MaxClock
  /\ clock' = clock + 1
  /\ UNCHANGED <<store, issued, bound, expiry, authorized, asked_resource, exchanges,
                 verifierAtAuthorize, sent>>

\* the user is sent to /authorize with a code challenge (never the verifier), a state, and the resource the
\* credential is for
Authorize(s, c, r) ==
  /\ authorized[c] = "none"
  /\ asked_resource[c] = "none"
  /\ authorized' = [authorized EXCEPT ![c] = s]
  /\ asked_resource' = [asked_resource EXCEPT ![c] = r]
  /\ UNCHANGED <<clock, store, issued, bound, expiry, exchanges, sent, verifierAtAuthorize>>

\* The counterfactual (D-412): a client that puts the PKCE verifier on the authorization URL — the URL the user
\* sees, the browser keeps in history and another machine may paste back. `MC_oauth_verifier_leaks.cfg` must refute
\* `VerifierNeverLeavesTheClient` with it enabled.
Leak ==
  /\ LeakVerifier
  /\ verifierAtAuthorize' = TRUE
  /\ UNCHANGED <<clock, store, issued, bound, expiry, authorized, asked_resource, exchanges, sent>>

\* the callback's code is exchanged for a token: the one place the verifier travels, and where the token is bound
\* to the resource the authorization request named
Exchange(s, c, t) ==
  /\ authorized[c] = s
  /\ (ReuseCode \/ exchanges[c] = 0)
  /\ t \notin issued
  /\ clock + MaxLife <= MaxClock
  /\ exchanges' = [exchanges EXCEPT ![c] = @ + 1]
  /\ issued' = issued \cup {t}
  /\ bound' = [bound EXCEPT ![t] = asked_resource[c]]
  /\ expiry' = [expiry EXCEPT ![t] = clock + MaxLife]
  /\ store' = [store EXCEPT ![s] = <<t, asked_resource[c]>>]
  /\ UNCHANGED <<clock, authorized, asked_resource, verifierAtAuthorize, sent>>

\* an MCP request presents the stored credential to an endpoint
Use(s, t, r) ==
  /\ t \in issued
  /\ (IgnoreLogout \/ \E r2 \in Resources : store[s] = <<t, r2>>)
  /\ (IgnoreResource \/ IgnoreLogout \/ store[s] = <<t, r>>)
  /\ (IgnoreExpiry \/ clock < expiry[t])
  /\ sent' = sent \cup {<<t, r, clock, s>>}
  /\ UNCHANGED <<clock, store, issued, bound, expiry, authorized, asked_resource, exchanges,
                 verifierAtAuthorize>>

\* an expired credential is renewed when the server issued a refresh token; the renewed token keeps the binding
Refresh(s, t, t2) ==
  /\ store[s] = <<t, bound[t]>>
  /\ s \in Refreshable
  /\ t \in issued
  /\ clock >= expiry[t]
  /\ t2 \notin issued
  /\ clock + MaxLife <= MaxClock
  /\ issued' = issued \cup {t2}
  /\ bound' = [bound EXCEPT ![t2] = bound[t]]
  /\ expiry' = [expiry EXCEPT ![t2] = clock + MaxLife]
  /\ store' = [store EXCEPT ![s] = <<t2, bound[t]>>]
  /\ UNCHANGED <<clock, authorized, asked_resource, exchanges, verifierAtAuthorize, sent>>

\* signing out deletes the credential: `Use` reads the store, so nothing is presented afterwards
Logout(s) ==
  /\ store[s] # SignedOut
  /\ store' = [store EXCEPT ![s] = SignedOut]
  /\ UNCHANGED <<clock, issued, bound, expiry, authorized, asked_resource, exchanges,
                 verifierAtAuthorize, sent>>

\* an idle step keeps the model open-ended (TLC then reports no false deadlock)
Stutter == UNCHANGED monVars

Next ==
  \/ Advance
  \/ Leak
  \/ \E s \in Services : \E c \in Codes : \E r \in Resources : Authorize(s, c, r)
  \/ \E s \in Services : \E c \in Codes : \E t \in Tokens : Exchange(s, c, t)
  \/ \E s \in Services : \E t \in Tokens : \E r \in Resources : Use(s, t, r)
  \/ \E s \in Services : \E t \in Tokens : \E t2 \in Tokens : Refresh(s, t, t2)
  \/ \E s \in Services : Logout(s)
  \/ Stutter   \* the system is open-ended: an idle step is always possible

Init ==
  /\ clock = 0
  /\ store = [ s \in Services |-> SignedOut ]
  /\ issued = {}
  /\ bound = [ t \in Tokens |-> "none" ]
  /\ expiry = [ t \in Tokens |-> 0 ]
  /\ authorized = [ c \in Codes |-> "none" ]
  /\ asked_resource = [ c \in Codes |-> "none" ]
  /\ exchanges = [ c \in Codes |-> 0 ]
  /\ verifierAtAuthorize = FALSE
  /\ sent = {}

Spec == Init /\ [][Next]_monVars

\* ------------------------------------------------------------- invariants --
TypeOK ==
  /\ clock \in 0..MaxClock
  /\ store \in [Services -> {SignedOut} \cup (Tokens \X Resources)]
  /\ issued \subseteq Tokens
  /\ bound \in [Tokens -> {"none"} \cup Resources]
  /\ expiry \in [Tokens -> 0..MaxClock]
  /\ authorized \in [Codes -> {"none"} \cup Services]
  /\ asked_resource \in [Codes -> {"none"} \cup Resources]
  /\ exchanges \in [Codes -> 0..2]
  /\ verifierAtAuthorize \in BOOLEAN
  /\ sent \subseteq Tokens \X Resources \X (0..MaxClock) \X Services

\* A token is presented only to the endpoint it was requested for: the RFC 8707 `resource` the authorization
\* request carried, repeated at the token endpoint and stored with the credential, is what makes a credential for
\* one MCP endpoint unusable at another.
TokenOnlyToItsResource ==
  \A e \in sent : bound[e[1]] = e[2]

\* The PKCE verifier is sent to the token endpoint only. It must never appear on the authorization URL — the URL
\* the user sees, the browser keeps in history and another machine may paste back.
VerifierNeverLeavesTheClient ==
  ~verifierAtAuthorize

\* An authorization code is exchanged once. A replayable code turns a leaked redirect URL into a second credential.
CodesAreSingleUse ==
  \A c \in Codes : exchanges[c] <= 1

\* No request presents a token past its expiry: an expired credential is refreshed (when the server issued a
\* refresh token) or the user signs in again — the client never guesses that a stale token might still work.
NoRequestWithAnExpiredToken ==
  \A e \in sent : e[3] < expiry[e[1]]

\* Signing out is effective: every step that presents a token for a service is a step where that service's stored
\* credential was that token. The control configuration that drops the guard must refute this.
LogoutIsEffective ==
  [][ \A s \in Services : \A t \in Tokens : \A r \in Resources :
        (\E at \in 0..MaxClock : <<t, r, at, s>> \in sent' \ sent) => store[s] = <<t, r>> ]_monVars

=============================================================================
