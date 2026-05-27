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
    let ncs = compile("void main() { int x = 1 + 2 - 3 * 4 / 5 % 6; }");
    assert!(has_opcode(&ncs, Opcode::Add));
    assert!(has_opcode(&ncs, Opcode::Sub));
    assert!(has_opcode(&ncs, Opcode::Mul));
    assert!(has_opcode(&ncs, Opcode::Div));
    assert!(has_opcode(&ncs, Opcode::Modulus));
}

#[test]
fn emits_comparison() {
    let ncs = compile("void main() { int a = 1 > 2; int b = 3 < 4; int c = 5 >= 6; int d = 7 <= 8; int e = 9 == 10; int f = 11 != 12; }");
    assert!(has_opcode(&ncs, Opcode::GT));
    assert!(has_opcode(&ncs, Opcode::LT));
    assert!(has_opcode(&ncs, Opcode::GEQ));
    assert!(has_opcode(&ncs, Opcode::LEQ));
    assert!(has_opcode(&ncs, Opcode::Equal));
    assert!(has_opcode(&ncs, Opcode::NotEqual));
}

#[test]
fn emits_logical_and_short_circuit() {
    let ncs = compile("void main() { int x = 1 && 0; }");
    assert!(has_opcode(&ncs, Opcode::Jz));
}

#[test]
fn emits_logical_or_short_circuit() {
    let ncs = compile("void main() { int x = 0 || 1; }");
    assert!(has_opcode(&ncs, Opcode::Jnz));
}

#[test]
fn emits_bitwise() {
    let ncs = compile("void main() { int x = 0xFF & 0x0F | 0xF0 ^ 0x55; }");
    assert!(has_opcode(&ncs, Opcode::BooleanAnd));
    assert!(has_opcode(&ncs, Opcode::InclusiveOr));
    assert!(has_opcode(&ncs, Opcode::ExclusiveOr));
}

#[test]
fn emits_shift() {
    let ncs = compile("void main() { int x = 1 << 2; int y = 8 >> 1; }");
    assert!(has_opcode(&ncs, Opcode::ShiftLeft));
    assert!(has_opcode(&ncs, Opcode::ShiftRight));
}

#[test]
fn emits_unary() {
    let ncs = compile("void main() { int x = -1; int y = !0; int z = ~0xFF; }");
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
fn do_while_emits_jnz() {
    let ncs = compile("void main() { int x = 0; do { x = 1; } while (0); }");
    assert!(has_opcode(&ncs, Opcode::Jnz));
}

#[test]
fn for_loop_compiles() {
    let ncs = compile("void main() { for (int i = 0; i < 10; i++) { } }");
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

// ===== Switch =====

#[test]
fn switch_compiles() {
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
    assert!(has_opcode(&ncs, Opcode::Jz));
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
            for (int i = 0; i < 10; i++) {
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
                    for (int i = 0; i < x; i++) {
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
