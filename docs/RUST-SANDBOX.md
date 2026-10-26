# Retired Rust-native sandbox design spike (D-466, superseded by D-480)

Historical evidence only. D-480 removed all OS sandbox backends at the user's request; this proposal
is closed and is not current implementation guidance or an approved next step.

Status: **design only**. No backend in this document is implemented, and nothing here claims that one works.
The pluggable selection key (`[permissions] sandbox`) already exists; this note decides what a Rust-native
backend behind it should be, and what it must prove before it may be offered.

## 1. What a backend owes the product

The current integration point is `SandboxKind` plus three functions in `engine/src/tools.rs`:
`probe_sandbox` (the honest availability check), `backend_available` (the cheap fail-closed gate on the dispatch
path) and `shell_command_spec` (argv construction, mounts, environment). A backend that cannot satisfy these
does not ship. Concretely a backend must:

1. **probe** without running a model command, and answer "usable here" honestly (kernel features, binaries,
   image, remote reachability);
2. **wrap** a command so the process starts inside the intended boundary, with the workspace mounted, the shell
   state directory writable, and model credentials excluded from the environment;
3. **describe** itself for `doctor` (one line, with the machine's own refusal text when it failed);
4. **fail closed**: a configured-but-unavailable backend refuses the command (`IsolationUnavailable`), it never
   silently degrades to the host;
5. **stop** the command's process group on timeout/cancellation, and leave no orphan behind.

The intended refactor, when a third implementation lands, is a `Sandbox` trait with these four operations and
`ShellMode::Sandboxed(Box<dyn Sandbox>)`. The enum dispatch is deliberate until then: one more implementation does
not justify an abstraction, and the trait extraction is mechanical.

## 2. Two candidate designs

### A. birdcage-style: landlock + seccomp, no namespaces, no root

`birdcage` (Rust) composes **Landlock** (filesystem scoping: allow the workspace and read-only system paths,
deny the rest) with **seccomp-BPF** (syscall filtering) and `setrlimit` bounds, all unprivileged. Landlock needs
Linux ≥ 5.13; TCP bind/connect rules need ≥ 6.7. No user namespace, so the AppArmor restriction that breaks
bubblewrap on Ubuntu 24.04 does not apply.

- **Gains**: works where unprivileged user namespaces are blocked (the CI runner, stock Ubuntu 24.04); no root;
  small surface; a real filesystem boundary (not just a mount view); seccomp can deny `socket`/`connect`.
- **Limits**: no PID/mount/network *namespace*; the process still sees host PIDs and `/proc`; no cgroup
  memory/CPU accounting (rlimits bound the process, not a group); network denial is syscall-level, not a
  per-destination allowlist; Landlock ABI varies by kernel, so the probe must name the ABI it got.

### B. DSec-style: namespaces + cgroups v2 + seccomp + Landlock

A self-built supervisor creates a user/mount/PID/network namespace set, joins a delegated cgroup v2 for
CPU/memory/pids, applies seccomp, and optionally an eBPF allowlist for egress — the shape DSec documents for
agentic training at scale.

- **Gains**: strong process/mount/network separation; honest resource ceilings; a per-destination network
  policy is possible; PID isolation hides other processes.
- **Limits**: the namespace half needs `CLONE_NEWUSER`, which is exactly what is blocked on Ubuntu 24.04 and the
  CI runner; without a setuid helper or a privileged container the design degrades to A. cgroups need a
  delegated subtree (`systemd-run --user --scope`), which is a host-service dependency; eBPF needs
  `CAP_BPF`/root on most kernels. This is the most capable and the most operationally fragile option.

## 3. Comparison

| Property | A: landlock + seccomp | B: namespaces + cgroups + seccomp |
|---|---|---|
| Root / setuid needed | no | no for cgroups-if-delegated, yes for eBPF; userns blocked on Ubuntu 24.04 |
| Works on the CI runner | likely (Landlock, no userns) | only the seccomp/landlock fallback |
| Filesystem boundary | allowlist via Landlock | mount namespace + allowlist |
| Network control | seccomp deny (coarse); Landlock TCP on ≥ 6.7 | netns + eBPF allowlist |
| CPU/memory ceilings | `setrlimit` per process | cgroup v2 per group |
| Process visibility | host PIDs visible | PID namespace |
| Complexity / dependencies | one crate + libc | supervisor, cgroup delegation, optional eBPF |
| Failure mode on old kernels | probe fails, backend refuses | falls back or refuses |

## 4. Recommended path

1. **Phase 2a (the backend worth building)**: `sandbox = "native"` = Landlock + seccomp + `setrlimit`, selected
   only when the probe finds a usable Landlock ABI. Filesystem allowlist: the workspace (read/write), the
   shell-state directory (read/write), system paths (read-only), nothing else. Network: `network = false`
   denies `socket`/`connect` at seccomp; `network = true` keeps them. No secrets in the child environment
   (the existing whitelist already enforces this).
2. **Phase 2b (only if a measured need appears)**: cgroups v2 for group memory/CPU, via a delegated
   `systemd-run --user --scope`; namespaces only on machines where the probe confirms `CLONE_NEWUSER` works.
3. Keep bubblewrap/docker/ssh as they are; `native` is a fourth value of the same key, not a replacement.

## 5. Acceptance plan (what would make this a backend, not a note)

- probe: on a kernel without Landlock, doctor says so and the command is refused (fail closed); on a supported
  kernel, the probe passes without running a model command;
- filesystem: writing inside the workspace succeeds; writing to `$HOME`, `/etc` and `/tmp/outside` (outside the
  allowed set) fails; reading `/etc/passwd` per the allowlist policy is either allowed-read or denied by design,
  and the test asserts the chosen rule;
- network: `network = false` makes `curl`/`getent` fail with the denial the probe names; `network = true`
  succeeds where the host does;
- resources: a `setrlimit` ceiling is observable (a runaway allocation/loop is killed, the receipt says so);
- cancellation: `instances terminate` / timeout stops the whole process group, no orphan survives;
- environment: a model credential in the parent environment never reaches the child;
- no root: the test runs as an unprivileged user and asserts no setuid binary is invoked.

## 6. Explicit non-claims

- Not a VM and not a kernel boundary: a kernel exploit escapes it, as it escapes bubblewrap.
- Not multi-tenant: it protects the host from the agent's filesystem/network reach, not two users from each
  other.
- Landlock coverage depends on the kernel ABI; the probe's reported ABI is part of the claim, and an older kernel
  means a weaker boundary, not a silent one.
- The design does not promise a per-domain network allowlist in Phase 2a; that is Phase 2b's eBPF work and it
  needs a separate decision.
- No macOS/Windows support is implied. This is Linux-only, like the rest of the product.
