# C++ Parity Status

Status of the Rust NWScript compiler's byte-for-byte / behavioral parity with the
original C++ compiler (`neverwinter/nwscript/native/`). Maintained during the
parity-hardening effort driven by multi-agent audit rounds.

Last updated: 2026-05-29.

## Where we are

- **Committed & pushed** (`rust-compiler`, commit `1b977ab`): all fixes from audit
  rounds 4–8. **205 Rust tests + JS/WASM suites green.** `project_integration`
  is 74/75 — the one failure is a known real `consts_roles.nss` bug, not a
  compiler defect.
- The audit method is a Workflow that fans out 5 focused auditors (codegen,
  semcheck, lexparse, stacksafety, realworld) which decode real WASM NCS bytes and
  compare against the C++ source, then a synthesis pass dedupes and triages.
  This caught blocking bugs that 30+ manual rounds and the error-count tests
  missed (the integration test never checked emitted bytecode).

### Fixed in rounds 4–8 (shipped)

Blocking runtime miscompiles:
- **const value substitution** — user `const` references in value expressions
  emitted *no code at all* (substitution was never implemented). Now folded to the
  literal, scope-aware so a local shadowing a const still wins.
- **`||` short-circuit** — used a `JNZ` that skipped the `LOGOR` opcode, leaving the
  raw truthy left operand on the stack. Now emits C++'s `COPY/JZ/COPY/JMP/LOGOR`.

Crash / stack-safety:
- Deep nested **ternary** bypassed the parse-depth guard → stack overflow / poisoned
  WASM instance. Guarded.
- Deep acyclic **struct** chains overflowed the default-value emitter (cycle guard
  but no depth cap). Added an anti-crash depth cap.

Semcheck parity:
- Struct-typed **ternary** lost its struct type name → spurious `-618`/`-587`/`-589`.
- Struct **return-type name** mismatch now rejected (`-620`).
- `++`/`--` on a `const` now rejected (consistent with `=`/`+=`).
- Global `void` declaration rejected (`-567`).
- Folded all-literal arithmetic (`2*3`) rejected as a default param (`-630`);
  `-5`/literals/bare consts still accepted.

Parser / lexer parity:
- Unary `+` accepted as a no-op prefix.
- Integer literals beyond i64 wrap to int32 (digit-wise) instead of becoming 0.
- `\x` escape with an invalid first nibble follows `strtol` semantics (→ 0).
- `#include` with a non-string argument falls through to declaration parsing
  instead of consuming the token + a spurious `FileNotFound`.
- Multi-variable `const` declarations (`const int A=1, B=2;`) accepted.
- Vector literal trailing comma rejected (`-631`).

## What's left

### Pending confirmed findings (NOT yet fixed)

Recovered from a rate-limited audit transcript and verified locally via WASM —
both currently compile with **0 errors** but should error:

1. **Undefined identifier as a default param** — `void f(int a = UNKNOWN){}` is
   accepted; should error.
2. **Type-mismatched const as a default param** — `void f(string a = TRUE){}`
   (`TRUE` is int) and `void f(int a = S){}` (`S` is a string const) are accepted;
   should be a type error (~`-630`).

Root cause: the documented bare-identifier default leniency (added for NWNX
`int base64 = TRUE`) is *too* loose — it exempts *any* `Variable` default
(`semcheck.rs` default-param validation, the `default_node.op != Operation::Variable`
branch), so undefined and wrong-typed identifiers slip through.

**Fix trap (read before implementing):** default-param validation runs in the
*first* registration pass (`collect_pass` → `register_func_decl`). Engine constants
(`load_lang_spec`) and included consts are pre-registered, but a **forward-referenced
same-file const is not** — `void f(int a=K); const int K=1;` currently compiles and
**must keep** compiling. A naive "reject if name unknown in `self.globals`" first-pass
check would break that and risk real NWNX headers. The correct fix resolves the
`Variable` default's identifier in a **second pass** (after all consts/globals are
registered): accept only a known const (`is_constant`) whose `nw_type` matches the
param; reject undefined names, non-const globals, and type mismatches. Gate on the
full fru `project_integration` (75 files) + forward-ref + engine-const
(`int a=TRUE`, `object a=OBJECT_INVALID`) before keeping. C++ source of truth:
`scriptcompparsetree.cpp:4040–4242`.

### Workflow hardening (TODO)

Round 9 reported a **false `CLEAN`**: 4/5 auditors hit an account session/rate
limit (HTTP 429) and were killed before calling `StructuredOutput`; the workflow's
`try/catch` swallowed each failure as "no findings". A hardened v2 script (each
auditor reports `{ok, coverage_complete}`; verdict is `CLEAN` only when
`failed=[]` **and** `incomplete=[]`, else `INCOMPLETE`) is drafted but not yet
saved. Re-run the audit loop after the rate limit resets, with the hardened script,
so a future `CLEAN` is trustworthy.

### Convergence signal

Actionable findings per round trended down: 10 → 7 → 5 → 3 → (1 blocking +
discovered const bug) → 1 → 2-high → and round 9 was inconclusive (rate-limited).
A genuinely clean run (all auditors completing with no actionable findings) has not
yet been achieved.

## Documented intentional divergences

See the "Known … divergences" sections of `README.md` for the full list (optimizer
strategy, initialized-local shape, float precision, lexer cosmetics, `#define`
support, const-in-arithmetic not pre-folded, `continue`-outside-loop stricter,
first-line self-`#include` dedup, etc.). These are settled and should not be
re-reported as bugs.
