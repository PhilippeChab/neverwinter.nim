# nwscript-compiler (Rust)

A NWScript compiler written in Rust, targeting WebAssembly for use in the [nwscript-ee-language-server](https://github.com/PhilippeChab/nwscript-ee-language-server) VS Code extension.

## Why

The original NWScript compiler is a ~19,000-line C++ codebase written by BioWare, wrapped in Nim by [neverwinter.nim](https://github.com/niv/neverwinter.nim). It works well as a CLI tool, but integrating it into a language server proved difficult:

- **C++/Emscripten WASM approach** ([wasm-lsp branch](https://github.com/PhilippeChab/neverwinter.nim/tree/wasm-lsp)): Required extensive multi-error recovery patches to the C++ code. The Emscripten JS runtime added 66KB of glue code, and the JS↔WASM boundary caused memory bugs (double-free in resolver callbacks).

- **Native binary approach** ([PR #77](https://github.com/PhilippeChab/nwscript-ee-language-server/pull/77)): Shelled out to `nwn_script_comp` as a child process. Required shipping 3 platform-specific binaries (~13MB), suffered from process leak bugs, and couldn't report multiple errors per file.

This Rust rewrite solves all of these problems by designing the compiler for LSP use from the start, with clean WASM output via `wasm-bindgen`.

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
node test_wasm.js
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
