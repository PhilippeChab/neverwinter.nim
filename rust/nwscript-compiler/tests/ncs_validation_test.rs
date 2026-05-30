use nwscript_compiler::compiler::{Compiler, CompilerOptions, MapResolver};
use nwscript_compiler::opcode::Opcode;

fn compile(src: &str) -> Vec<u8> {
    let c = Compiler::new(CompilerOptions {
        require_entry_point: false,
        ..Default::default()
    });
    let r = MapResolver::new();
    let result = c.compile(src, "test.nss", &r);
    assert!(
        result.success,
        "Compilation failed: {:?}",
        result.diagnostics
    );
    assert!(!result.ncs.is_empty(), "No NCS output");
    result.ncs
}

fn compile_with_spec(src: &str, spec: &str) -> Vec<u8> {
    let mut c = Compiler::new(CompilerOptions {
        require_entry_point: false,
        ..Default::default()
    });
    c.set_language_spec(spec);
    let r = MapResolver::new();
    let result = c.compile(src, "test.nss", &r);
    assert!(
        result.success,
        "Compilation failed: {:?}",
        result.diagnostics
    );
    result.ncs
}

fn has_opcode(ncs: &[u8], op: Opcode) -> bool {
    ncs.contains(&(op as u8))
}

fn count_opcode(ncs: &[u8], op: Opcode) -> usize {
    ncs.iter().filter(|&&b| b == op as u8).count()
}

fn ncs_header_valid(ncs: &[u8]) -> bool {
    ncs.len() >= 13
        && &ncs[0..8] == b"NCS V1.0"
        && ncs[8] == b'B'
        && {
            let size = i32::from_be_bytes([ncs[9], ncs[10], ncs[11], ncs[12]]);
            size as usize == ncs.len()
        }
}

// ===== Header validation =====

// Run a deep-recursion regression on a thread with a large stack. The depth GUARDS
// are sized for the release/WASM target (1 MB stack, small frames — verified handling
// these cases without crashing). The native *debug* test thread has only ~2 MB with
// much larger unoptimized frames, so it would overflow at the guarded depth purely as
// a build artifact; a generous stack lets the test exercise the real guard logic.
fn run_big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_struct_field_read_does_not_overflow() {
    // Regression: gen_struct_field_read_typed self-recursed unguarded over the
    // .field chain, bypassing the generate_expr depth guard and crashing ~3500 deep.
    run_big_stack(|| {
        const N: usize = 2100; // comfortably past the 2000 expr-depth guard
        let mut src = String::from("struct S0 { int x; };\n");
        for i in 1..=N {
            src.push_str(&format!("struct S{} {{ struct S{} a; }};\n", i, i - 1));
        }
        src.push_str(&format!("void main() {{ struct S{} v; v", N));
        for _ in 0..N { src.push_str(".a"); }
        src.push_str(".x; }\n");
        let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
        let r = MapResolver::new();
        let _ = c.compile(&src, "t.nss", &r); // must return, not overflow
    });
}

#[test]
fn long_unary_chain_does_not_overflow() {
    // Regression: parse_unary_expr recursed unguarded on prefix operators.
    run_big_stack(|| {
        let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
        let r = MapResolver::new();
        let src = format!("void main() {{ int q = {}1; }}", "!".repeat(50000));
        let _ = c.compile(&src, "t.nss", &r); // must return cleanly, not overflow
    });
}

#[test]
fn deep_ternary_chain_does_not_overflow() {
    // Regression: parse_ternary_expr recursed directly via its then/else branches,
    // bypassing the shared parse-depth guard, so a deep `1?1:1?1:...` chain overflowed
    // the stack (poisoning the WASM instance). Must now return a clean diagnostic.
    run_big_stack(|| {
        let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
        let r = MapResolver::new();
        let src = format!("void main() {{ int x = {}1; }}", "1?1:".repeat(50000));
        let res = c.compile(&src, "t.nss", &r);
        assert!(!res.success, "deep ternary chain should error, not overflow");
    });
}

#[test]
fn unary_plus_is_accepted() {
    // C++ UNARY_EXPRESSION rule 6: a leading unary `+` is a no-op prefix.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    assert!(c.compile("void main() { int x = +5; }", "t.nss", &r).success);
}

#[test]
fn global_void_declaration_is_rejected() {
    // C++ FUNCTIONAL_UNIT rule: a void-typed global declaration is INVALID_DECLARATION_TYPE.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    assert!(!c.compile("void v;\nvoid main() {}", "t.nss", &r).success);
    // A void function must still compile.
    assert!(c.compile("void main() {}", "t.nss", &r).success);
}

#[test]
fn logical_or_normalizes_via_logor_opcode() {
    // Regression: `||` used a JNZ that skipped the LOGOR opcode, leaving the raw truthy
    // left operand (e.g. 5) on the stack instead of the normalized 1. C++ emits
    // COPY; JZ; COPY; JMP; <right>; LOGOR — the LOGOR (0x07) must always be reached.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    let res = c.compile("void main(){ int x=5; int y=0; int z = x || y; }", "t.nss", &r);
    assert!(res.success);
    let ncs = &res.ncs;
    // LOGOR opcode (0x07) must be present; the broken version emitted JNZ (0x25) for ||.
    assert!(ncs.iter().any(|&b| b == 0x07), "LOGOR opcode must be emitted for ||");
    // The first JZ after a CPTOPSP for the || must carry the C++ jump length of +20.
    // Locate CPTOPSP(0x03) immediately followed by JZ(0x1f) and check its offset.
    let mut found_jz20 = false;
    let mut i = 0;
    while i + 8 <= ncs.len() {
        if ncs[i] == 0x03 && i + 8 + 6 <= ncs.len() && ncs[i + 8] == 0x1f {
            let off = i32::from_be_bytes([ncs[i + 10], ncs[i + 11], ncs[i + 12], ncs[i + 13]]);
            if off == 20 { found_jz20 = true; break; }
        }
        i += 1;
    }
    assert!(found_jz20, "|| JZ must jump +20 to match C++ short-circuit layout");
}

#[test]
fn struct_return_type_name_must_match() {
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    // Returning a differently-named struct from a struct-returning function -> -620.
    let bad = c.compile(
        "struct A{int x;}; struct B{int y;}; struct A make(){ struct B b; return b; } void main(){ make(); }",
        "t.nss", &r,
    );
    assert!(!bad.success, "mismatched struct return name must be rejected");
    // Same struct name is fine.
    let ok = c.compile(
        "struct A{int x;}; struct A make(){ struct A a; return a; } void main(){ make(); }",
        "t.nss", &r,
    );
    assert!(ok.success);
}

#[test]
fn integer_literal_wraps_to_int32() {
    // C++ accumulates each digit into int32 with wrapping; out-of-i64 literals must keep
    // their low 32 bits rather than becoming 0.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    let res = c.compile("void main(){ int x = 0xFFFFFFFFFFFFFFFF; }", "t.nss", &r);
    assert!(res.success);
    // CONST INT (0x04 0x03) payload must be -1, not 0.
    let ncs = &res.ncs;
    let mut got = None;
    let mut i = 0;
    while i + 6 <= ncs.len() {
        if ncs[i] == 0x04 && ncs[i + 1] == 0x03 {
            got = Some(i32::from_be_bytes([ncs[i + 2], ncs[i + 3], ncs[i + 4], ncs[i + 5]]));
            break;
        }
        i += 1;
    }
    assert_eq!(got, Some(-1), "0xFFFFFFFFFFFFFFFF must wrap to -1");
}

#[test]
fn include_with_non_string_falls_through() {
    // C++ ignores a #include whose argument is not a string and does NOT consume the
    // token or emit FileNotFound; the token falls through to declaration parsing.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
    let r = MapResolver::new();
    // #include at EOF is a clean no-op.
    let res = c.compile("void main(){}\n#include", "t.nss", &r);
    assert!(res.success, "trailing bare #include should be a no-op, not an error");
}

// Helper: collect every CONST INT (0x04 0x03) payload in order.
fn const_ints(ncs: &[u8]) -> Vec<i32> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 6 <= ncs.len() {
        if ncs[i] == 0x04 && ncs[i + 1] == 0x03 {
            v.push(i32::from_be_bytes([ncs[i + 2], ncs[i + 3], ncs[i + 4], ncs[i + 5]]));
            i += 6;
        } else {
            i += 1;
        }
    }
    v
}

#[test]
fn const_reference_folds_to_its_value() {
    // Regression for a blocking miscompile: a user `const` referenced in a value
    // expression emitted NO code (codegen had no const values), so callers received
    // garbage. C++ folds the const to its literal at the use site.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    // const passed as an argument must materialize the value 7.
    let res = c.compile("const int A=7;\nvoid f(int x){}\nvoid main(){ f(A); }", "t.nss", &r);
    assert!(res.success);
    assert!(const_ints(&res.ncs).contains(&7), "const arg must emit CONST 7, got {:?}", const_ints(&res.ncs));
    // const returned must materialize the value 11.
    let res2 = c.compile("const int A=11;\nint f(){ return A; } void main(){ f(); }", "t.nss", &r);
    assert!(const_ints(&res2.ncs).contains(&11), "const return must emit CONST 11");
    // const-from-const resolves transitively.
    let res3 = c.compile("const int A=11;\nconst int B=A;\nint f(){ return B; } void main(){ f(); }", "t.nss", &r);
    assert!(const_ints(&res3.ncs).contains(&11), "const-from-const must emit 11");
    // A local shadowing a const must NOT be folded to the const value.
    let res4 = c.compile("const int A=7;\nint f(){ int A = 3; return A; } void main(){ f(); }", "t.nss", &r);
    assert!(const_ints(&res4.ncs).contains(&3) && !const_ints(&res4.ncs).contains(&7),
        "shadowing local must win over const, got {:?}", const_ints(&res4.ncs));
}

#[test]
fn multi_variable_const_declaration() {
    // C++ accepts a comma-separated const list; each declarator gets its own value.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    let res = c.compile(
        "const int A=11, B=22, C=33;\nint a(){ return A; } int b(){ return B; } int cc(){ return C; }\nvoid main(){ a(); b(); cc(); }",
        "t.nss", &r,
    );
    assert!(res.success, "multi-var const must compile: {:?}", res.diagnostics);
    let ks = const_ints(&res.ncs);
    assert!(ks.contains(&11) && ks.contains(&22) && ks.contains(&33),
        "each multi-var const must fold to its own value, got {:?}", ks);
}

#[test]
fn const_increment_is_rejected() {
    // The project's const-mutation rule (applied to `=`/`+=`) must also cover ++/--.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    for src in [
        "const int A=5;\nvoid main(){ A++; }",
        "const int A=5;\nvoid main(){ ++A; }",
        "const int A=5;\nvoid main(){ A--; }",
    ] {
        assert!(!c.compile(src, "t.nss", &r).success, "const mutation via ++/-- must be rejected: {src}");
    }
    // A non-const local ++ is still fine.
    assert!(c.compile("void main(){ int i = 0; i++; }", "t.nss", &r).success);
}

#[test]
fn struct_typed_ternary_keeps_struct_name() {
    // Regression: a struct-typed ternary lost its struct type name, causing spurious
    // -618/-587/-589 when used as an assignment RHS, argument, or equality operand.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
    let r = MapResolver::new();
    let ok = [
        "struct S{int x;}; void main(){ int cc; struct S a,b,r; r = cc ? a : b; }",
        "struct S{int x;}; void use(struct S s){} void main(){ int cc; struct S a,b; use(cc?a:b); }",
        "struct S{int x;}; void main(){ int cc; struct S a,b; int z = (cc?a:b)==a; }",
        "struct S{int x;}; void main(){ int cc; struct S a,b,r; r = cc?(cc?a:b):a; }",
    ];
    for src in ok {
        assert!(c.compile(src, "t.nss", &r).success, "struct ternary should be accepted: {src}");
    }
    // Mismatched struct branches must still be rejected.
    assert!(!c.compile(
        "struct S{int x;}; struct T{int y;}; void main(){ int cc; struct S a; struct T b,r; r = cc ? a : b; }",
        "t.nss", &r).success);
}

#[test]
fn deep_acyclic_struct_default_value_does_not_overflow() {
    // Regression: emit_default_value_named_guarded recursed once per struct nesting
    // level with only a cycle guard (no depth cap), overflowing the WASM stack on a
    // deep acyclic chain. Must return cleanly now.
    run_big_stack(|| {
        const N: usize = 8000;
        let mut src = format!("struct S{} {{ int f; }};\n", N);
        for i in (0..N).rev() {
            src.push_str(&format!("struct S{} {{ struct S{} f; }};\n", i, i + 1));
        }
        src.push_str("struct S0 g;\nvoid main(){}");
        let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
        let r = MapResolver::new();
        let _ = c.compile(&src, "t.nss", &r); // must not overflow
    });
}

#[test]
fn folded_arithmetic_rejected_as_default_param() {
    // C++ validates a default value's node shape before folding, so `2*3` is rejected
    // even though it folds to a constant; `-5` (unary) and bare literals stay valid.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, ..Default::default() });
    let r = MapResolver::new();
    assert!(!c.compile("void foo(int a = 2*3){}\nvoid main(){foo();}", "t.nss", &r).success);
    assert!(!c.compile("void foo(int a = 1+2){}\nvoid main(){foo();}", "t.nss", &r).success);
    assert!(c.compile("void foo(int a = 6){}\nvoid main(){foo();}", "t.nss", &r).success);
    assert!(c.compile("void foo(int a = -5){}\nvoid main(){foo();}", "t.nss", &r).success);
    // But folded arithmetic is still fine in a const declaration (C++ folds those).
    assert!(c.compile("const int A = 2*3;\nint f(){return A;} void main(){f();}", "t.nss", &r).success);
}

#[test]
fn engine_call_args_pushed_right_to_left() {
    // Regression: engine (EXECUTE_COMMAND) call args must be pushed right-to-left so the
    // first declared parameter ends on TOP of the stack (the NWScript engine ABI). A
    // void-returning prototype loaded as the language spec is treated as an engine action.
    let spec = "void Eng(int first, int second);";
    let ncs = compile_with_spec(
        "void main() { Eng(11, 22); }",
        spec,
    );
    // The two CONST INT operands should appear as 22 then 11 (second pushed first, first
    // pushed last/on top). Find the two CONST INT (0x04 0x03) payloads in order.
    let mut consts = Vec::new();
    let mut i = 0;
    while i + 6 <= ncs.len() {
        if ncs[i] == Opcode::Constant as u8 && ncs[i + 1] == 0x03 {
            consts.push(i32::from_be_bytes([ncs[i + 2], ncs[i + 3], ncs[i + 4], ncs[i + 5]]));
            i += 6;
        } else {
            i += 1;
        }
    }
    assert_eq!(consts, vec![22, 11], "engine args must push second(22) before first(11)");
}

#[test]
fn struct_equality_emits_correct_size_operand() {
    // Regression: `s1 == s2` emitted a 0 size operand (and corrupted stack tracking)
    // because the Variable LHS has no type_name on the AST node. Must emit the real
    // struct byte size (2 ints = 8) after EQUAL aux 0x24.
    let ncs = compile("struct S { int a; int b; }; void main() { struct S x; struct S y; int r = (x == y); }");
    // Find EQUAL (0x0b) with aux 0x24 and check the following u16 size == 8.
    let mut found = false;
    for i in 0..ncs.len().saturating_sub(3) {
        if ncs[i] == Opcode::Equal as u8 && ncs[i + 1] == 0x24 {
            let sz = u16::from_be_bytes([ncs[i + 2], ncs[i + 3]]);
            assert_eq!(sz, 8, "struct== size operand should be 8");
            found = true;
        }
    }
    assert!(found, "no EQUAL/STRUCT_STRUCT opcode emitted");
}

#[test]
fn deep_global_initializer_does_not_overflow() {
    // Regression: check_initializer_refs recursed unguarded on global initializers,
    // which bypass the expression depth guard — a deep `1+1+...` overflowed the stack.
    run_big_stack(|| {
        let mut e = String::from("1");
        for _ in 0..15000 { e.push_str("+1"); }
        let src = format!("int g = {};\nvoid main() {{}}", e);
        let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
        let r = MapResolver::new();
        let _ = c.compile(&src, "t.nss", &r); // must return, not overflow
    });
}

#[test]
fn header_empty_main() {
    let ncs = compile("void main() { }");
    assert!(ncs_header_valid(&ncs));
}

#[test]
fn header_complex_script() {
    let ncs = compile(r#"
        int add(int a, int b) { return a + b; }
        void main() { int x = add(1, 2); }
    "#);
    assert!(ncs_header_valid(&ncs));
}

#[test]
fn mutually_recursive_structs_do_not_overflow() {
    // Regression: A-contains-B / B-contains-A by value would recurse forever in
    // emit_default_value_named. Must not crash (a cycle guard breaks it).
    let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
    let r = MapResolver::new();
    let result = c.compile(
        "struct A { struct B b; int x; }; struct B { struct A a; int y; }; void main() { struct A a; }",
        "t.nss",
        &r,
    );
    // Whatever the diagnostics, the key property is: it returned without overflowing.
    let _ = result;
}

#[test]
fn pathological_nesting_errors_cleanly() {
    // Deeply nested parens / blocks must NOT overflow the parser stack; the depth
    // guard turns them into a clean error so the (WASM) instance stays usable.
    let c = Compiler::new(CompilerOptions { require_entry_point: false, collect_all_errors: true, ..Default::default() });
    let r = MapResolver::new();
    let src = format!("void main() {{ int x = {}1{}; }}", "(".repeat(5000), ")".repeat(5000));
    let result = c.compile(&src, "t.nss", &r);
    assert!(!result.success && !result.diagnostics.is_empty());
}

#[test]
fn large_function_body_does_not_overflow_stack() {
    // Regression: StatementList chains (function bodies) were walked recursively in
    // generate_stmt / check_statement / all_paths_return — overflowing at ~2000 stmts.
    let mut body = String::new();
    for _ in 0..3000 {
        body.push_str("x = x + 1;\n");
    }
    let ncs = compile(&format!("void main() {{ int x = 0;\n{}}}", body));
    assert!(ncs_header_valid(&ncs));
}

#[test]
fn large_switch_does_not_overflow_stack() {
    // Regression: switch-body chains were walked recursively in flatten_switch_items /
    // check_switch_cases — overflowing at ~500 cases.
    let mut cases = String::new();
    for i in 0..700 {
        cases.push_str(&format!("case {}: break;\n", i));
    }
    let ncs = compile(&format!("void main() {{ int x = 0; switch (x) {{\n{}}} }}", cases));
    assert!(ncs_header_valid(&ncs));
}

#[test]
fn large_program_does_not_overflow_stack() {
    // Regression: the top-level FunctionalUnit chain was walked recursively
    // (one stack frame per declaration) in resolve_includes and four codegen
    // passes — overflowing on large includes like nwscript.nss (~2000+ decls).
    // 3000 globals exceeds the old crash threshold.
    let mut src = String::new();
    for i in 0..3000 {
        src.push_str(&format!("int g{} = {};\n", i, i));
    }
    src.push_str("void main() { int x = g0 + g2999; }\n");
    let ncs = compile(&src);
    assert!(ncs_header_valid(&ncs));
}

// ===== Determinism =====

#[test]
fn deterministic_simple() {
    let src = "void main() { int x = 1 + 2; }";
    assert_eq!(compile(src), compile(src));
}

#[test]
fn deterministic_50x() {
    let src = "void main() { int a = 0; while (a < 10) { a = a + 1; } }";
    let first = compile(src);
    for _ in 0..49 {
        assert_eq!(compile(src), first);
    }
}

// ===== Instruction presence =====

#[test]
fn emits_const_int() {
    let ncs = compile("void main() { int x = 42; }");
    assert!(ncs.windows(2).any(|w| w[0] == Opcode::Constant as u8 && w[1] == 0x03));
}

#[test]
fn emits_const_float() {
    let ncs = compile("void main() { float f = 3.14; }");
    assert!(ncs.windows(2).any(|w| w[0] == Opcode::Constant as u8 && w[1] == 0x04));
}

#[test]
fn emits_const_string() {
    let ncs = compile(r#"void main() { string s = "hello"; }"#);
    assert!(ncs.windows(2).any(|w| w[0] == Opcode::Constant as u8 && w[1] == 0x05));
    assert!(ncs.windows(5).any(|w| w == b"hello"));
}

#[test]
fn emits_const_object() {
    let ncs = compile("void main() { object o = OBJECT_SELF; }");
    assert!(ncs.windows(2).any(|w| w[0] == Opcode::Constant as u8 && w[1] == 0x06));
}

#[test]
fn emits_arithmetic() {
    // Use non-constant operands so the constant folder does not collapse the expression.
    let ncs = compile("void main() { int n = 7; int x = n + 2 - n * 4 / 5 % n; }");
    assert!(has_opcode(&ncs, Opcode::Add));
    assert!(has_opcode(&ncs, Opcode::Sub));
    assert!(has_opcode(&ncs, Opcode::Mul));
    assert!(has_opcode(&ncs, Opcode::Div));
    assert!(has_opcode(&ncs, Opcode::Modulus));
}

#[test]
fn emits_comparison() {
    // Use a non-literal operand so the constant folder doesn't collapse the comparisons.
    let ncs = compile("void main() { int n = 7; int a = n > 2; int b = n < 4; int c = n >= 6; int d = n <= 8; int e = n == 10; int f = n != 12; }");
    assert!(has_opcode(&ncs, Opcode::GT));
    assert!(has_opcode(&ncs, Opcode::LT));
    assert!(has_opcode(&ncs, Opcode::GEQ));
    assert!(has_opcode(&ncs, Opcode::LEQ));
    assert!(has_opcode(&ncs, Opcode::Equal));
    assert!(has_opcode(&ncs, Opcode::NotEqual));
}

#[test]
fn emits_logical_and_opcode() {
    // C++: `&&` emits short-circuit JZ + LogicalAnd opcode (0x06).
    // Use a non-literal so const folding doesn't collapse the expression.
    let ncs = compile("void main() { int n = 1; int x = n && 0; }");
    assert!(has_opcode(&ncs, Opcode::LogicalAnd));
}

#[test]
fn emits_logical_or_opcode() {
    let ncs = compile("void main() { int n = 0; int x = n || 1; }");
    assert!(has_opcode(&ncs, Opcode::LogicalOr));
}

#[test]
fn emits_bitwise() {
    // Every operand must be non-literal or const folding collapses the expression.
    let ncs = compile("void main() { int n = 0xFF; int x = (n & n) | (n ^ n); }");
    assert!(has_opcode(&ncs, Opcode::BooleanAnd));
    assert!(has_opcode(&ncs, Opcode::InclusiveOr));
    assert!(has_opcode(&ncs, Opcode::ExclusiveOr));
}

#[test]
fn emits_shift() {
    let ncs = compile("void main() { int n = 1; int x = n << 2; int y = n >> 1; }");
    assert!(has_opcode(&ncs, Opcode::ShiftLeft));
    assert!(has_opcode(&ncs, Opcode::ShiftRight));
}

#[test]
fn emits_unary() {
    // Use a non-literal operand so the constant folder doesn't collapse the unary op.
    let ncs = compile("void main() { int n = 5; int x = -n; int y = !n; int z = ~n; }");
    assert!(has_opcode(&ncs, Opcode::Negation));
    assert!(has_opcode(&ncs, Opcode::BooleanNot));
    assert!(has_opcode(&ncs, Opcode::OnesComplement));
}

#[test]
fn emits_increment_decrement() {
    let ncs = compile("void main() { int x = 0; x++; x--; }");
    assert!(has_opcode(&ncs, Opcode::Increment));
    assert!(has_opcode(&ncs, Opcode::Decrement));
}

// ===== Control flow =====

#[test]
fn if_emits_jz() {
    let ncs = compile("void main() { if (1) { int x = 1; } }");
    assert!(has_opcode(&ncs, Opcode::Jz));
}

#[test]
fn if_else_emits_jmp() {
    let ncs = compile("void main() { if (1) { int x = 1; } else { int y = 2; } }");
    assert!(has_opcode(&ncs, Opcode::Jz));
    assert!(has_opcode(&ncs, Opcode::Jmp));
}

#[test]
fn while_emits_jz_and_jmp() {
    let ncs = compile("void main() { while (0) { } }");
    assert!(has_opcode(&ncs, Opcode::Jz));
    assert!(has_opcode(&ncs, Opcode::Jmp));
}

#[test]
fn while_body_is_emitted() {
    // Regression: a plain `while` body was previously dropped because gen_while
    // passed the WhileChoice wrapper (not the body) to generate_stmt, which had
    // no arm for it. The body's arithmetic must appear in the bytecode.
    let ncs = compile("void main() { int x = 5; int y = 0; while (x > 0) { y = y + 1; x = x - 1; } }");
    assert!(has_opcode(&ncs, Opcode::Add), "while body add missing — body dropped");
    assert!(has_opcode(&ncs, Opcode::Sub), "while body sub missing — body dropped");
    // A loop whose body actually emits is substantially longer than an empty one.
    let empty = compile("void main() { int x = 5; while (x > 0) { } }");
    assert!(ncs.len() > empty.len() + 8, "while body produced no extra bytecode");
}

#[test]
fn do_while_emits_jnz() {
    let ncs = compile("void main() { int x = 0; do { x = 1; } while (0); }");
    assert!(has_opcode(&ncs, Opcode::Jnz));
}

#[test]
fn for_loop_compiles() {
    // C++ NWScript does not allow declarations in for-init; pre-declare the counter.
    let ncs = compile("void main() { int i; for (i = 0; i < 10; i++) { } }");
    assert!(has_opcode(&ncs, Opcode::Jz));
    assert!(has_opcode(&ncs, Opcode::Jmp));
}

// ===== Functions =====

#[test]
fn function_emits_ret() {
    let ncs = compile("void foo() { }");
    assert!(has_opcode(&ncs, Opcode::Ret));
}

#[test]
fn multiple_functions_multiple_rets() {
    let ncs = compile("void foo() { } void bar() { }");
    assert!(count_opcode(&ncs, Opcode::Ret) >= 3); // loader + foo + bar
}

#[test]
fn function_call_emits_jsr() {
    let ncs = compile("void foo() { } void main() { foo(); }");
    assert!(has_opcode(&ncs, Opcode::Jsr));
}

#[test]
fn jsr_targets_are_resolved() {
    let ncs = compile("void foo() { } void main() { foo(); }");
    for i in 0..ncs.len().saturating_sub(5) {
        if ncs[i] == Opcode::Jsr as u8 && ncs[i + 1] == 0 {
            let offset = i32::from_be_bytes([ncs[i + 2], ncs[i + 3], ncs[i + 4], ncs[i + 5]]);
            assert_ne!(offset, 0, "JSR at byte {} has unresolved offset", i);
        }
    }
}

#[test]
fn engine_function_emits_execute_command() {
    let ncs = compile_with_spec(
        "void main() { VoidFn(); }",
        "void VoidFn();",
    );
    assert!(has_opcode(&ncs, Opcode::ExecuteCommand));
}

// ===== Global variables =====

#[test]
fn global_var_emits_save_base_pointer() {
    let ncs = compile("int gCount = 0;\nvoid main() { }");
    assert!(has_opcode(&ncs, Opcode::SaveBasePointer));
    assert!(has_opcode(&ncs, Opcode::RestoreBasePointer));
}

// ===== Variable access =====

#[test]
fn local_var_emits_runstack_copy() {
    let ncs = compile("void main() { int x = 1; int y = x; }");
    assert!(has_opcode(&ncs, Opcode::RunstackCopy));
}

#[test]
fn assignment_emits_assignment_op() {
    let ncs = compile("void main() { int x = 0; x = 42; }");
    assert!(has_opcode(&ncs, Opcode::Assignment));
}

// ===== Ternary =====

#[test]
fn ternary_emits_jz_and_jmp() {
    let ncs = compile("void main() { int x = (1 > 0) ? 1 : 0; }");
    assert!(has_opcode(&ncs, Opcode::Jz));
    assert!(has_opcode(&ncs, Opcode::Jmp));
}

#[test]
fn ternary_struct_result_rolls_back_full_struct_size() {
    // C++ scriptcompfinalcode.cpp:2392-2398 rolls the compile-time stack back by the
    // then-branch result's full size between the two branches (GetStructureSize for a
    // struct). A 3-int struct is 12 bytes, so the else-branch copy of `y` (which sits
    // immediately above `x` on the stack) must use offset -12, not -20 (which would be
    // the result of rolling back only 4 bytes). Verify both arms copy 12 bytes from the
    // correct, identical-depth locations.
    let ncs = compile(
        "struct S { int a; int b; int c; }; \
         void main() { int cond = 1; struct S x; struct S y; struct S z = cond ? x : y; }",
    );
    // Locate the two 12-byte RUNSTACK_COPY instructions (the two ternary arms).
    // RUNSTACK_COPY = 0x03, aux 0x01, i32 offset, u16 size=0x000c.
    let mut copies: Vec<i32> = Vec::new();
    let mut i = 0usize;
    while i + 8 <= ncs.len() {
        if ncs[i] == Opcode::RunstackCopy as u8
            && ncs[i + 1] == 0x01
            && i16::from_be_bytes([ncs[i + 6], ncs[i + 7]]) == 12
        {
            copies.push(i32::from_be_bytes([ncs[i + 2], ncs[i + 3], ncs[i + 4], ncs[i + 5]]));
        }
        i += 1;
    }
    assert_eq!(copies, vec![-24, -12], "ternary struct arms: {:?}", copies);
}

// ===== Switch =====

#[test]
fn switch_compiles() {
    // C++ NWScript emits a two-pass switch: COPYTOP+CONST+EQUAL+JNZ per case,
    // then a final JMP to default/exit, then the bodies. Verify Equal and Jnz.
    let ncs = compile(r#"
        void main() {
            int x = 1;
            switch (x) {
                case 0: break;
                case 1: break;
                default: break;
            }
        }
    "#);
    assert!(has_opcode(&ncs, Opcode::Equal));
    assert!(has_opcode(&ncs, Opcode::Jnz));
}

// ===== NDB output =====

#[test]
fn debug_output_generated_when_requested() {
    let c = Compiler::new(CompilerOptions {
        require_entry_point: false,
        generate_debug: true,
        ..Default::default()
    });
    let r = MapResolver::new();
    let result = c.compile("void main() { }", "test.nss", &r);
    assert!(!result.ndb.is_empty(), "NDB should be generated");
    let ndb_text = String::from_utf8(result.ndb).unwrap();
    assert!(ndb_text.starts_with("NDB V1.0"));
    assert!(ndb_text.contains("test.nss"));
}

#[test]
fn no_debug_output_by_default() {
    let c = Compiler::new(CompilerOptions {
        require_entry_point: false,
        ..Default::default()
    });
    let r = MapResolver::new();
    let result = c.compile("void main() { }", "test.nss", &r);
    assert!(result.ndb.is_empty());
}

// ===== Comprehensive scripts =====

#[test]
fn complex_script_compiles() {
    let ncs = compile(r#"
        struct Vec2 { int x; int y; };

        const int MAX_COUNT = 100;

        int add(int a, int b) { return a + b; }

        int factorial(int n) {
            if (n <= 1) { return 1; }
            return n * factorial(n - 1);
        }

        void main() {
            int sum = 0;
            int i; for (i = 0; i < 10; i++) {
                sum = add(sum, i);
            }

            int f = factorial(5);

            if (sum > 20 && f > 100) {
                int x = sum + f;
            } else {
                int y = sum - f;
            }

            int z = (sum > 0) ? sum : -sum;

            switch (z % 3) {
                case 0: break;
                case 1: z = z + 1; break;
                default: z = z - 1; break;
            }
        }
    "#);
    assert!(ncs_header_valid(&ncs));
    assert!(ncs.len() > 50);
}

#[test]
fn all_literal_types_compile() {
    let ncs = compile(r#"
        void main() {
            int a = 42;
            int b = 0xFF;
            int c = 0b1010;
            int d = 0o77;
            float f = 3.14;
            string s = "hello\nworld";
            object o = OBJECT_SELF;
            object inv = OBJECT_INVALID;
        }
    "#);
    assert!(ncs_header_valid(&ncs));
}

#[test]
fn nested_control_flow_compiles() {
    let ncs = compile(r#"
        void main() {
            int x = 0;
            while (x < 10) {
                if (x % 2 == 0) {
                    int i; for (i = 0; i < x; i++) {
                        if (i > 3) {
                            break;
                        }
                    }
                } else {
                    do {
                        x = x + 1;
                    } while (x < 5);
                }
                x = x + 1;
            }
        }
    "#);
    assert!(ncs_header_valid(&ncs));
}

#[test]
fn include_with_compilation() {
    let mut c = Compiler::new(CompilerOptions {
        require_entry_point: false,
        ..Default::default()
    });
    c.set_language_spec("void PrintString(string s);");
    let mut r = MapResolver::new();
    r.add_file("utils", "int double(int n) { return n * 2; }");
    let result = c.compile(
        "#include \"utils\"\nvoid main() { int x = double(21); }",
        "main.nss",
        &r,
    );
    assert!(result.success, "{:?}", result.diagnostics);
    assert!(ncs_header_valid(&result.ncs));
}

#[test]
fn ndb_populated_with_function_and_variable_entries() {
    let mut c = Compiler::new(CompilerOptions {
        require_entry_point: false,
        generate_debug: true,
        ..Default::default()
    });
    let r = MapResolver::new();
    let result = c.compile(
        "int add(int a, int b) { int c = a + b; return c; }\nvoid main() { int x = add(1, 2); }",
        "test.nss",
        &r,
    );
    let ndb = String::from_utf8(result.ndb).expect("NDB should be valid UTF-8");
    assert!(ndb.starts_with("NDB V1.0"));
    assert!(ndb.contains("N00 test.nss"));
    // Should have function entries for add and main
    assert!(ndb.contains(" add"), "NDB missing add function: {}", ndb);
    assert!(ndb.contains(" main"), "NDB missing main function: {}", ndb);
    // Should have variable entries for c, x
    assert!(ndb.contains("i c") || ndb.contains("i x"), "NDB missing var entries: {}", ndb);
    // Should have line entries — C++ format is "l%02d %07d ..." (no space after l).
    assert!(ndb.contains("l00 "), "NDB missing line entries: {}", ndb);
}
