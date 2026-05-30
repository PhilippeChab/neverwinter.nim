# nwscript-compiler (Rust)

A NWScript compiler written in Rust, targeting WebAssembly for use in the [nwscript-ee-language-server](https://github.com/PhilippeChab/nwscript-ee-language-server) VS Code extension.

## Why

The [nwscript-ee-language-server](https://github.com/PhilippeChab/nwscript-ee-language-server) needed a compiler that runs in-process as WASM — cross-platform, no native binaries to ship, multi-error reporting, and an AST query API for IDE features (go-to-definition, completions, signature help). Rust's first-class WASM support via `wasm-bindgen` made it the right tool for this.

## ⚠ Not for shipping NCS to the game

This compiler is built for **LSP diagnostics** — catching errors as you type, providing AST queries for IDE features. The NCS bytecode it emits is structurally correct but has **not** been verified byte-for-byte against `nwn_script_comp` and has **not** been tested in the NWN:EE virtual machine.

**Use `nwn_script_comp` (the official C++ compiler) to compile scripts you actually intend to run on a game server or in a module.** This Rust compiler is the right tool for editor tooling; it is not yet the right tool for producing shippable game artifacts.

### Known optimizer-strategy divergence from C++

The Rust compiler uses post-emission peephole optimizations (`optimize.rs`), while the C++
compiler performs parse-tree pruning and emission-time pattern matching:

- **`OPTIMIZE_DEAD_BRANCHES`**: C++ removes dead branches from the parse tree *before* codegen
  (`scriptcompfinalcode.cpp::TrimParseTree`). Rust emits the dead branch and then NOPs out the
  `CONSTI + JZ` prologue. The dead code remains in the binary but is unreachable.
- **`OPTIMIZE_MELD_INSTRUCTIONS`**: C++ matches patterns like `RUNSTACK_ADD + CONST + ASSIGN +
  MOVSP` and rewrites them inline during emission. Rust only merges adjacent `MODIFY_STACK_POINTER`
  instructions in a post-pass.

The result is functionally equivalent but the byte-level layout differs from C++. For LSP
diagnostic use this has no impact; for byte-equivalent NCS production, use `nwn_script_comp`.

### Known bytecode-shape divergences (functionally equivalent)

A few constructs emit byte-different but semantically equivalent NCS:

- **Initialized locals**: C++ emits `RUNSTACK_ADD + CONST + ASSIGNMENT + MODIFY_STACK_POINTER`
  for `int n = 3;`; Rust just runs `generate_expr(init)` and treats the pushed value as the
  variable's slot. Runtime stack state matches; the byte sequence does not.
- **No token-length limit**: C++ enforces `CSCRIPTCOMPILER_MAX_TOKEN_LENGTH = 65536`. Rust does
  not (extreme edge case — only matters for pathological inputs).
- **`continue` outside loop**: C++ silently emits a JMP that later fails symbol resolution.
  Rust raises `BreakOutsideOfLoopOrCaseStatement` up front (friendlier; different error path).
- **Float literal precision**: C++ parses fractional literals incrementally in `f32`
  (`scriptcomplexical.cpp::ParseFloatFromTokenString`), accumulating rounding error per digit.
  Rust uses `str::parse::<f32>()` which does IEEE-754 round-to-nearest-even. The resulting
  float values are within 1 ULP and indistinguishable at runtime, but bit-level byte
  comparisons of NCS against `nwn_script_comp` can differ for some literal values.
- **Multi-return function shape**: C++ emits a single function-exit label (`FX_<name>`) that
  all `return` statements `JMP` to; the label's body does `MODIFY_STACK_POINTER + RET`. Rust
  emits the `MODIFY_STACK_POINTER + RET` sequence inline at each `return`. Functionally
  equivalent at runtime; byte sequence differs for functions with more than one return.
- **Do-while loop shape**: C++ emits `JZ +6 ; JMP -X` (condition false → skip back-jump,
  condition true → JMP back to loop top). Rust emits a single `JNZ -X` (jump back when
  condition is true). Same byte budget, same semantics, different opcode pattern.
- **Integer divide-by-zero in constants**: C++ `ConstantFoldNode` runs `result = left / right`
  unconditionally and crashes the compiler with SIGFPE on `1/0` / `0/0`. Rust skips the fold
  for `b == 0`, defers the divide to runtime, and the VM traps there. Strictly safer at
  compile time; identical runtime behaviour.
- **`#identifier` error code**: a stray `#` followed by an identifier raises
  `UnexpectedCharacter` (-560) in Rust vs `EllipsisInIdentifier` (-602) in C++. Both reject;
  only the error code differs.
- **`# include` whitespace**: C++ accepts whitespace between `#` and a directive
  (`# include "foo"` works). Rust requires the directive name to be contiguous. Real-world
  code never uses the spaced form.
- **Hex literal case in source text**: C++ normalises `0xABCDEF` to lowercase in the token
  text (`0xabcdef`); Rust preserves the source case. Numeric value parses identically;
  diagnostics that echo the literal show the casing difference.
- **Undefined struct type in a declaration**: C++ raises `UndefinedStructure` (-611) for
  `struct Undef u;`. Rust intentionally stays silent here — the LSP routinely sees struct
  types defined in `#include` files it has not loaded, and flagging them would produce false
  positives on otherwise-valid code. (This mirrors the existing "struct definition unknown —
  silent" leniency in the semantic checker.)
- **Short-circuit constant folding** (`1 || expr`, `0 && expr`): C++ folds these to the
  determining constant at codegen time, discarding the (never-evaluated) right operand. Rust
  folds `&&`/`||` only when *both* operands are constant; for `1 || expr` it emits the normal
  short-circuit sequence (which also never evaluates `expr` at runtime). Rust deliberately does
  not discard the right operand, because its constant folder runs *before* semantic analysis —
  discarding it would skip type-checking of that subexpression. Runtime behaviour is identical;
  only the emitted bytecode shape differs.
- **`continue` outside a loop**: C++ has no dedicated check for a stray `continue` (only `break`
  is validated), so it emits a dangling continue label and misbehaves downstream. Rust raises
  `BreakOutsideOfLoopOrCaseStatement` (-4834) up front — stricter and friendlier, since a stray
  `continue` is always a real bug. Kept intentionally rather than reproducing C++'s buggy path.
- **Default `#include` depth limit**: Rust defaults `max_include_depth` to 200 (the C++
  `MAX_INCLUDE_LEVELS` ceiling) where shipping C++ defaults to 16. The LSP host can set either;
  the higher default avoids false "too many include levels" errors on deep real-world include
  graphs. Configurable via `CompilerOptions::max_include_depth`.
- **Object-like `#define` macros**: Rust supports `#define NAME VALUE` in user scripts (the
  preprocessor expands `NAME` → `VALUE` before lexing). The stock C++ compiler has no
  user-facing `#define` and would reject it as a parse error. This is an intentional editor-side
  convenience; it does not affect parity for the (vast majority of) scripts that don't use it.
- **Struct definition and the `#globals` loader wrapper**: C++ adds a `struct { … };`
  definition to its global-variable parse tree, which makes `InstallLoader` emit the
  `#globals` wrapper (`SAVE_BASE_POINTER` … `RESTORE_BASE_POINTER` + a `JSR #globals`) even
  when the file declares no actual global *variables*. Rust emits that wrapper only when a
  real global variable exists. For a type-only struct definition the wrapper is a functional
  no-op (no global data is allocated), so runtime behaviour is identical — only the loader
  byte layout differs. (User-function call argument order differs similarly: C++ pushes
  right-to-left with first-parameter-on-top; Rust pushes user-call args left-to-right and
  assigns user parameter offsets to match, which is self-consistent at runtime. Engine
  (`EXECUTE_COMMAND`) calls DO push right-to-left to satisfy the fixed engine ABI.)
- **Bare-identifier default parameter values** (`void f(int a = SOME_CONST)`): C++ folds
  predefined constants (`TRUE`, `OBJECT_INVALID`, …) to literals at lex time, so the default
  there is a literal node; a *non-folded* user constant would be rejected (-630). Rust keeps
  such references as `Variable` nodes and accepts any in-scope constant/global as a default.
  This matches real shipping code (NWNX headers use `int base64 = TRUE`,
  `int x = NWNX_RENAME_PLAYERNAME_DEFAULT`, etc.); rejecting bare identifiers would
  false-positive on valid headers, so Rust intentionally accepts them.
- **Stacked prefix unary operators**: C++ allows a prefix operator's operand to be another
  prefix expression only when re-entered through `!` (`PRIMARY_EXPRESSION` special-cases
  `BOOLEAN_NOT`); other direct nestings like `~~5`, `!-5`, `-~5`, `- -5` hit a parser error.
  Rust's `parse_unary_expr` recurses uniformly, so it accepts any stacking. This is an
  over-acceptance of pathological input only — real scripts never stack prefix operators —
  and the exact C++ accept/reject matrix for these cases is inconsistent enough that matching
  it byte-for-byte would add fragile grammar special-casing for zero real-world benefit.
- **Mid-identifier raw/hashed-string prefix** (`abcr"foo"`): the C++ lexer fires its `r"`/`h"`
  raw-string rule even in the middle of an identifier, silently *discarding* the pending
  identifier so `abcr"foo"` lexes as the string `"foo"`. Rust's lexer only recognizes the
  `r"`/`h"` prefix at a token boundary, so it reads `abcr` as an identifier and then errors on
  the adjacent string. Reproducing C++'s silent-identifier-discard here would be actively
  user-hostile (it hides a typo); the divergence is confined to this pathological lexing edge.
- **`const`-in-arithmetic not pre-folded**: a user `const` referenced in a value
  expression is folded to its literal at the use site (matching C++ — consts allocate no
  storage), so direct uses (`f(A)`, `g = A`, `case A`, `return A`) are byte-identical. But
  C++ replaces a const reference with a `CONSTANT` node in the parse tree *before* its
  constant-folder runs, so `A + 1` (const `A`) collapses to a single `CONST 12`. Rust folds
  per-file *before* const values are known and substitutes scope-aware at codegen time
  (so a local shadowing a const still wins — which a blind AST rewrite would break), so
  `A + 1` emits `CONST A; CONST 1; ADD`. Runtime-identical; only the bytecode shape of
  const-bearing *arithmetic* differs, and a scope-correct pre-fold would add a full extra
  scope-tracking pass for no runtime gain.
- **No 65536-char token-length limit**: C++ caps any single token at
  `CSCRIPTCOMPILER_MAX_TOKEN_LENGTH` (65536) and returns `ERROR_TOKEN_TOO_LONG` (-610) past it.
  Rust's lexer enforces no such cap (the `-610` variant exists but is never emitted), so a
  pathological >64 KB identifier/number/string that C++ rejects would lex fine. Only triggers on
  inputs that never occur in real scripts; left unenforced for simplicity.
- **First-line self-`#include` of the entry file**: Rust seeds the include-dedup set with the
  entry file's own resref before resolving includes, so a `#include "self"` of the entry file is
  always silently deduped. Stock C++ registers the entry file's parse-tree name *lazily*, so a
  self-include on line 1 (before any declaration) misses the dedup and trips the recursive-include
  check (-604). Rust's behavior is the more consistent one (a self-include anywhere is deduped),
  and the divergence depends on a C++ registration-ordering artifact on pathological input.

## Built by

This compiler was implemented by [Claude Code](https://claude.ai/claude-code) (Claude Opus 4.7), Anthropic's AI coding agent, in collaboration with [Philippe Chabot](https://github.com/PhilippeChab). The full implementation — lexer, parser, semantic checker, code generator, WASM bindings, BIF/KEY reader, and test suite — was built and iterated on over a single extended session, tested against a real NWScript project (75 files, 0 errors).

## Features

- **Full NWScript lexer** — 125 token types, all literal forms (hex, binary, octal, raw strings, hashed strings)
- **Recursive descent parser** — error recovery, multi-variable declarations, full NWScript grammar
- **Semantic analysis** — type checking, symbol table, multi-error collection, struct field resolution
- **NCS bytecode codegen** — constants, arithmetic, comparison, logical/bitwise operators, control flow, function calls (JSR + EXECUTE_COMMAND), compound assignments, switch/case, global variables
- **`#include` resolution** — cycle detection, depth limiting, cross-file type checking and error reporting
- **Language spec loading** — parses the full 12,600-line `nwscript.nss` with `#define ENGINE_STRUCTURE_N` directives
- **BIF/KEY resource reading** — reads stock NWN scripts from game archives
- **Optimization passes** — dead branch removal, instruction melding
- **NDB debug output** — line mappings, function/variable entries
- **AST JSON export** — full parse tree serialization for IDE features
- **Position queries** — `findNodeAtPosition`, `getDefinitionAtPosition`, `isInFunctionCall`, `getFunctionNameAtPosition`, `getActiveParameterIndex`, `getCompletionsAtPosition`
- **WASM bindings** — 250KB binary (89KB gzipped), no Emscripten runtime

## WASM API

```javascript
const { WasmCompiler } = require('./pkg/nwscript_compiler.js');

const c = new WasmCompiler();
c.setLanguageSpec(nwscriptNssContent);
c.setCollectAllErrors(true);
c.setRequireEntryPoint(false);

c.addFile('mylib', '#include "utils"\nint helper() { return 1; }');
c.addFile('main', '#include "mylib"\nvoid main() { int x = helper(); }');

const code = c.compile('main');
// code === 0 on success

// Diagnostics
for (let i = 0; i < c.getCollectedErrorCount(); i++) {
    console.log(c.getCollectedError(i));       // "file.nss(10): ERROR: ..."
    console.log(c.getCollectedErrorSeverity(i)); // 0 = error, 1 = warning
}

// NCS output
const ncs = c.getNcsBytes(); // Uint8Array

// AST for IDE features
const ast = JSON.parse(c.getParseTreeJSON());
const node = JSON.parse(c.findNodeAtPosition(5, 10));
const def = JSON.parse(c.getDefinitionAtPosition(5, 10));
const inCall = c.isInFunctionCall(5, 15);
const funcName = c.getFunctionNameAtPosition(5, 15);
const paramIdx = c.getActiveParameterIndex(5, 15);

c.free();
```

## Building

```bash
# Native tests
cargo test

# WASM build
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target nodejs --out-dir pkg \
    target/wasm32-unknown-unknown/release/nwscript_compiler.wasm

# Run WASM tests
node tests/wasm_test.js
```

Requires:
- Rust 1.80+ with `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)
- `wasm-bindgen-cli` (`cargo install wasm-bindgen-cli`)

## Tests

- **175 Rust unit/integration tests** — lexer, parser, semantic checker, codegen, NCS validation, BIF/KEY reading, AST queries
- **30 Node.js WASM tests** — end-to-end compilation, error collection, NCS output, AST export, position queries
- **75-file project integration test** — compiles every file from a real NWScript project with the full `nwscript.nss` language spec

## Architecture

```
src/
├── lexer.rs        Tokenizer (125 token types)
├── parser.rs       Recursive descent parser → AST
├── semcheck.rs     Type checker + symbol table
├── codegen.rs      AST → NCS bytecode
├── compiler.rs     Orchestration: lex → parse → check → codegen
├── wasm.rs         wasm-bindgen API surface
├── astquery.rs     AST JSON export + position queries
├── ast.rs          AST node types + arena allocator
├── token.rs        Token type definitions
├── types.rs        NwType system (int, float, string, struct, engine structures)
├── errors.rs       Error codes + Diagnostic with severity
├── opcode.rs       NCS VM opcodes
├── optimize.rs     Dead branch removal + instruction melding
├── ndb.rs          Debug symbol output
└── resources.rs    BIF/KEY binary archive reader
```

## License

GPL-3.0, matching the upstream NWScript compiler source.
