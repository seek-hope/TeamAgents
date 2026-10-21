# Claims without a refuting control

`review/verification_catalogue.py` requires every claim a positive configuration checks to be
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
