# Claims without a refuting control

`review/verification_catalogue.py` requires every **behavioural** claim a positive configuration checks to be
either the `expects:` target of a counterexample configuration (that control is then shown to
refute it, D-462) or a row here with a status. **0** claims are in this state today: they are
enumerated exhaustively by TLC, but no configuration is shown to break them, so a claim that
became vacuous would still verify. The number is recomputed and compared by the audit; it can only
go down by adding a control (and then deleting the row) or by replacing the row with a justified
status: `entailed-by: <Claim>` (the named claim is refuted by a control and implies this one) or
`structural: <reason>` (a property of the definition, not of a reachable state).

Regenerate with `python3 review/verification_catalogue.py --write-coverage`.

| Module | Claim | Status |
|---|---|---|
| `V2Retry` | `ZeroMeansOneAttempt` | entailed-by: NoAttemptBeyondTheBudget |
| `V2Store` | `EveryRefusalIsSilent` | entailed-by: RefusalIsSilent |
| `V2Store` | `NoForeignStampAlways` | entailed-by: NoForeignStamp |
| `V2Store` | `RefusalsWriteNothing` | entailed-by: RefusalIsSilent |

## Composite type claims (excluded from behavioural counterfactuals)

A `TypeOK*` predicate says the module's variables stay inside their declared ranges. A violation is
a defect in the model's own next-state relation, which TLC reports as an error rather than a reachable
product state (that is exactly how D-462 found one control refuting `TypeOK`), so no `expects:` control
targets one. This list is the whole exclusion: the audit fails if a skipped claim is missing here, or if
a row here is not a `TypeOK*` claim.

| Module | Composite claim |
|---|---|
| `V2Approval` | `TypeOK` |
| `V2Artifact` | `TypeOK` |
| `V2Authority` | `TypeOK` |
| `V2Checks` | `TypeOK` |
| `V2Codemode` | `TypeOK` |
| `V2Compress` | `TypeOK` |
| `V2Concurrency` | `TypeOK` |
| `V2Control` | `TypeOK` |
| `V2Coordinator` | `TypeOK` |
| `V2Daemon` | `TypeOK` |
| `V2DiskFull` | `TypeOK` |
| `V2DisplayWorker` | `TypeOK` |
| `V2Grants` | `TypeOK` |
| `V2Grants` | `TypeOKGrants` |
| `V2Grants` | `TypeOKOffered` |
| `V2Grants` | `TypeOKOps` |
| `V2Grants` | `TypeOKRevision` |
| `V2History` | `TypeOK` |
| `V2Images` | `TypeOK` |
| `V2Inbox` | `TypeOK` |
| `V2InputQueue` | `TypeOK` |
| `V2Isolation` | `TypeOK` |
| `V2Jobs` | `TypeOK` |
| `V2Memory` | `TypeOK` |
| `V2Oauth` | `TypeOK` |
| `V2Presentation` | `TypeOK` |
| `V2ProjectTrust` | `TypeOK` |
| `V2Prompt` | `TypeOK` |
| `V2Prune` | `TypeOK` |
| `V2Retention` | `TypeOK` |
| `V2Retry` | `TypeOK` |
| `V2Schedule` | `TypeOK` |
| `V2SessionUi` | `TypeOK` |
| `V2Sessions` | `TypeOK` |
| `V2Store` | `TypeOK` |
| `V2Task` | `TypeOK` |
| `V2Trust` | `TypeOK` |
| `V2Tui` | `TypeOK` |
| `V2Update` | `TypeOK` |
| `V2Wait` | `TypeOK` |
| `V2Workspace` | `TypeOK` |
