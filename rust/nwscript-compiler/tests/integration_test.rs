use nwscript_compiler::compiler::{Compiler, CompilerOptions, MapResolver};
use nwscript_compiler::errors::CompileError;

const LANG_SPEC: &str = r#"
int Nonsense(int n);
int IntFn(int n);
string StringFn(string s);
void VoidFn();
"#;

fn make_compiler(collect_all: bool, require_entry: bool) -> Compiler {
    let mut c = Compiler::new(CompilerOptions {
        require_entry_point: require_entry,
        collect_all_errors: collect_all,
        ..Default::default()
    });
    c.set_language_spec(LANG_SPEC);
    c
}

// ===================== Happy path =====================

#[test]
fn valid_void_main_compiles_cleanly() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile("void main() { int x = 1 + 2; }", "hello.nss", &r);
    assert!(
        result.diagnostics.is_empty(),
        "Expected no errors, got: {:?}",
        result.diagnostics
    );
    assert!(result.success);
}

#[test]
fn global_vars_and_helper_no_main_with_flag_on() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile(
        "int gCount = 0;\nint helper(int n) { return n + gCount; }",
        "inc.nss",
        &r,
    );
    assert!(
        result.diagnostics.is_empty(),
        "Expected no errors, got: {:?}",
        result.diagnostics
    );
}

// ===================== Single-error mode =====================

#[test]
fn undefined_identifier_reports_name() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile("void main() { DoesNotExist(); }", "p.nss", &r);
    assert!(!result.success);
    assert!(!result.diagnostics.is_empty());
    let msg = &result.diagnostics[0].message;
    assert!(
        msg.contains("DoesNotExist") || result.diagnostics[0].error == CompileError::UndefinedIdentifier,
        "Expected undefined identifier error, got: {}",
        msg
    );
}

#[test]
fn type_mismatch_reports_error() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile("void main() { string s = 42; }", "p.nss", &r);
    assert!(!result.success);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.error == CompileError::MismatchedTypes));
}

#[test]
fn missing_main_reports_error() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile("int helper(int n) { return n; }", "p.nss", &r);
    assert!(!result.success);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.error == CompileError::NoFunctionMainInScript));
}

// ===================== No-entry-point flag =====================

#[test]
fn include_only_errors_when_flag_off() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile("int helper(int n) { return n; }", "inc.nss", &r);
    assert!(!result.success);
}

#[test]
fn include_only_compiles_when_flag_on() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile("int helper(int n) { return n; }", "inc.nss", &r);
    assert!(
        result.diagnostics.is_empty(),
        "Expected no errors, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn script_with_main_works_when_flag_on() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile("void main() { int x = 1; }", "p.nss", &r);
    assert!(result.success);
}

// ===================== Multi-error mode =====================

#[test]
fn three_type_errors_in_three_functions() {
    let c = make_compiler(true, false);
    let r = MapResolver::new();
    let result = c.compile(
        r#"
void main() { string s = 42; }
void other() { int i = "hello"; }
void third() { int j = "x"; }
"#,
        "p.nss",
        &r,
    );
    let type_errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.error == CompileError::MismatchedTypes)
        .collect();
    assert_eq!(
        type_errors.len(),
        3,
        "Expected 3 type errors, got {} total: {:?}",
        type_errors.len(),
        result.diagnostics
    );
}

#[test]
fn two_undefined_in_two_functions() {
    let c = make_compiler(true, false);
    let r = MapResolver::new();
    let result = c.compile(
        r#"
void main() { DoesNotExistA(); }
void other() { DoesNotExistB(); }
"#,
        "p.nss",
        &r,
    );
    let undef_errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.error == CompileError::UndefinedIdentifier)
        .collect();
    assert_eq!(undef_errors.len(), 2);
}

#[test]
fn three_undefined_in_three_functions() {
    let c = make_compiler(true, false);
    let r = MapResolver::new();
    let result = c.compile(
        r#"
void main() { DoesNotExistA(); }
void other() { DoesNotExistB(); }
void third() { DoesNotExistC(); }
"#,
        "p.nss",
        &r,
    );
    let undef_errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.error == CompileError::UndefinedIdentifier)
        .collect();
    assert_eq!(undef_errors.len(), 3);
}

#[test]
fn valid_script_in_multi_error_mode_no_errors() {
    let c = make_compiler(true, false);
    let r = MapResolver::new();
    let result = c.compile("void main() { int x = 1; }", "p.nss", &r);
    assert!(result.success);
    assert!(result.diagnostics.is_empty());
}

// ===================== NWScript language features =====================

#[test]
fn struct_with_fields() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile(
        r#"
struct MyStruct {
    int x;
    float y;
    string name;
};
"#,
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn function_with_optional_params() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile(
        r#"
void DoThing(int a, int b = 10, string c = "hello") { }
"#,
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn control_flow_constructs() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        r#"
void main() {
    int x = 0;
    if (x > 0) { x = 1; } else { x = -1; }
    while (x < 10) { x = x + 1; }
    do { x = x - 1; } while (x > 0);
    int i; for (i = 0; i < 5; i++) { x = x + i; }
    switch (x) {
        case 0: break;
        case 1: break;
        default: break;
    }
}
"#,
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn ternary_expression() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "void main() { int x = (1 > 0) ? 1 : 0; }",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn object_constants() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "void main() { object o = OBJECT_SELF; object inv = OBJECT_INVALID; }",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn const_global() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile(
        "const int MY_CONST = 42;\nconst string MY_STR = \"hello\";",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn hex_binary_octal_literals() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "void main() { int a = 0xFF; int b = 0b1010; int c = 0o77; }",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn unary_operators() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "void main() { int x = -1; int y = !0; int z = ~0xFF; }",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn pre_post_increment() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "void main() { int x = 0; x++; ++x; x--; --x; }",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn function_calling_lang_spec_function() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "void main() { int x = IntFn(42); }",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

// ===================== Include resolution =====================

#[test]
fn include_resolves_function() {
    let c = make_compiler(false, true);
    let mut r = MapResolver::new();
    r.add_file("lib", "int helper(int n) { return n * 2; }");
    let result = c.compile(
        "#include \"lib\"\nvoid main() { int x = helper(3); }",
        "main.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn include_file_not_found() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "#include \"nonexistent\"\nvoid main() { }",
        "main.nss",
        &r,
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.error == CompileError::FileNotFound));
}

#[test]
fn chained_includes() {
    let c = make_compiler(false, true);
    let mut r = MapResolver::new();
    r.add_file("base", "int base_fn(int n) { return n; }");
    r.add_file(
        "mid",
        "#include \"base\"\nint mid_fn(int n) { return base_fn(n) + 1; }",
    );
    let result = c.compile(
        "#include \"mid\"\nvoid main() { int x = mid_fn(5); }",
        "main.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn recursive_include_detected() {
    let c = make_compiler(true, false);
    let mut r = MapResolver::new();
    r.add_file("a", "#include \"b\"\nint fa() { return 1; }");
    r.add_file("b", "#include \"a\"\nint fb() { return 2; }");
    let result = c.compile("#include \"a\"", "main.nss", &r);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.error == CompileError::IncludeRecursive));
}

#[test]
fn include_struct_visible_in_main() {
    let c = make_compiler(false, true);
    let mut r = MapResolver::new();
    r.add_file("types", "struct Vec2 { int x; int y; };");
    let result = c.compile(
        "#include \"types\"\nvoid main() { }",
        "main.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn include_global_visible_in_main() {
    let c = make_compiler(false, true);
    let mut r = MapResolver::new();
    r.add_file("globals", "int gCounter = 0;");
    let result = c.compile(
        "#include \"globals\"\nvoid main() { int x = gCounter; }",
        "main.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

// ===================== Cross-file body type checking =====================

#[test]
fn error_in_included_function_body() {
    let c = make_compiler(true, false);
    let mut r = MapResolver::new();
    r.add_file("bad_lib", "void helper() { int x = \"wrong\"; }");
    let result = c.compile(
        "#include \"bad_lib\"",
        "main.nss",
        &r,
    );
    assert!(
        result.diagnostics.iter().any(|d| d.error == CompileError::MismatchedTypes),
        "Expected type error from included file body: {:?}",
        result.diagnostics
    );
}

#[test]
fn error_in_included_file_reports_correct_filename() {
    let c = make_compiler(true, false);
    let mut r = MapResolver::new();
    r.add_file("mylib", "void broken() { int x = \"oops\"; }");
    let result = c.compile(
        "#include \"mylib\"",
        "main.nss",
        &r,
    );
    let type_err = result
        .diagnostics
        .iter()
        .find(|d| d.error == CompileError::MismatchedTypes);
    assert!(type_err.is_some(), "Expected type error: {:?}", result.diagnostics);
    assert!(
        type_err.unwrap().file.contains("mylib"),
        "Error should reference mylib, got: {}",
        type_err.unwrap().file
    );
}

#[test]
fn errors_in_main_and_included_file() {
    let c = make_compiler(true, false);
    let mut r = MapResolver::new();
    r.add_file("lib", "void lib_fn() { string s = 42; }");
    let result = c.compile(
        "#include \"lib\"\nvoid main() { int x = \"bad\"; }",
        "main.nss",
        &r,
    );
    let type_errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.error == CompileError::MismatchedTypes)
        .collect();
    assert_eq!(
        type_errors.len(),
        2,
        "Expected 2 type errors (one in lib, one in main): {:?}",
        result.diagnostics
    );
}

#[test]
fn valid_included_file_produces_no_errors() {
    let c = make_compiler(true, true);
    let mut r = MapResolver::new();
    r.add_file("good_lib", "int helper(int n) { return n * 2; }");
    let result = c.compile(
        "#include \"good_lib\"\nvoid main() { int x = helper(3); }",
        "main.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

// ===================== Edge cases =====================

#[test]
fn empty_source() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile("", "test.nss", &r);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn only_comments() {
    let c = make_compiler(false, false);
    let r = MapResolver::new();
    let result = c.compile(
        "// this is a comment\n/* block comment */",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn multiple_assignment_operators() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        r#"
void main() {
    int x = 0;
    x += 1;
    x -= 1;
    x *= 2;
    x /= 2;
    x %= 3;
    x &= 0xFF;
    x |= 0x0F;
    x ^= 0xF0;
    x <<= 2;
    x >>= 1;
}
"#,
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn deeply_nested_expressions() {
    let c = make_compiler(false, true);
    let r = MapResolver::new();
    let result = c.compile(
        "void main() { int x = ((1 + 2) * (3 - 4)) / ((5 % 6) + 7); }",
        "test.nss",
        &r,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}
