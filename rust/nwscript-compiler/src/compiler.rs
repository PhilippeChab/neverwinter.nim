use std::collections::HashSet;

use crate::ast::{AstArena, NodeId, NULL_NODE, Operation};
use crate::codegen::CodeGenerator;
use crate::errors::{CompileError, Diagnostic, Severity};
use crate::lexer::set_engine_structures;
use crate::lexer::Lexer;
use crate::ndb::NdbBuilder;
use crate::optimize;
use crate::parser::Parser;
use crate::semcheck::SemanticChecker;

pub struct CompileResult {
    pub success: bool,
    pub ncs: Vec<u8>,
    pub ndb: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
}

pub trait FileResolver {
    fn resolve(&self, filename: &str) -> Option<String>;
}

pub struct MapResolver {
    files: std::collections::HashMap<String, String>,
}

impl MapResolver {
    pub fn new() -> Self {
        Self {
            files: std::collections::HashMap::new(),
        }
    }

    pub fn add_file(&mut self, name: &str, content: &str) {
        self.files.insert(name.to_string(), content.to_string());
    }
}

impl Default for MapResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl FileResolver for MapResolver {
    fn resolve(&self, filename: &str) -> Option<String> {
        self.files.get(filename).cloned()
    }
}

pub struct CompilerOptions {
    pub require_entry_point: bool,
    pub collect_all_errors: bool,
    pub generate_debug: bool,
    pub optimization_level: u32,
    pub max_include_depth: u32,
    pub max_errors: usize,
}

impl Default for CompilerOptions {
    fn default() -> Self {
        Self {
            require_entry_point: true,
            collect_all_errors: false,
            generate_debug: false,
            optimization_level: 0,
            max_include_depth: 200,
            max_errors: 100,
        }
    }
}

struct ParsedFile {
    arena: AstArena,
    root: NodeId,
    file_names: Vec<String>,
    diagnostics: Vec<Diagnostic>,
}

/// Expand simple #define NAME VALUE macros and strip the directives.
/// Doesn't handle function-like macros — NWScript only uses object-like ones.
pub fn preprocess_defines(source: &str) -> String {
    use std::collections::HashMap;
    let mut macros: HashMap<String, String> = HashMap::new();
    let mut out = String::with_capacity(source.len());

    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#define") {
            let rest = rest.trim_start();
            // Split into name and value
            let mut parts = rest.splitn(2, char::is_whitespace);
            if let Some(name) = parts.next() {
                let value = parts.next().unwrap_or("").trim().to_string();
                if !name.is_empty() && is_valid_macro_name(name) {
                    macros.insert(name.to_string(), value);
                }
            }
            // Replace the line with blank so line numbers stay aligned
            out.push('\n');
            continue;
        }

        // Expand macros in this line — word-boundary aware
        let expanded = expand_macros_in_line(line, &macros);
        out.push_str(&expanded);
        out.push('\n');
    }
    out
}

fn is_valid_macro_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn expand_macros_in_line(line: &str, macros: &std::collections::HashMap<String, String>) -> String {
    if macros.is_empty() { return line.to_string(); }

    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut in_string = false;
    let mut in_line_comment = false;

    while i < bytes.len() {
        let c = bytes[i];

        // Track string/comment state to avoid expansion inside them
        if in_line_comment {
            out.push(c as char);
            i += 1;
            continue;
        }
        if in_string {
            out.push(c as char);
            if c == b'"' && (i == 0 || bytes[i-1] != b'\\') {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_string = true;
            out.push(c as char);
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i+1] == b'/' {
            in_line_comment = true;
            out.push('/');
            i += 1;
            continue;
        }

        // Try to match an identifier at this position
        let is_ident_start = c.is_ascii_alphabetic() || c == b'_';
        let prev_is_ident = i > 0 && (bytes[i-1].is_ascii_alphanumeric() || bytes[i-1] == b'_');

        if is_ident_start && !prev_is_ident {
            let mut end = i;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let ident = &line[i..end];
            if let Some(val) = macros.get(ident) {
                out.push_str(val);
                i = end;
                continue;
            }
        }

        out.push(c as char);
        i += 1;
    }
    out
}

fn parse_engine_structure_defines(spec: &str) {
    let mut mappings: Vec<(u8, String)> = Vec::new();
    for line in spec.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#define ENGINE_STRUCTURE_") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                if let Some(idx_str) = parts[1].strip_prefix("ENGINE_STRUCTURE_") {
                    if let Ok(idx) = idx_str.parse::<u8>() {
                        mappings.push((idx, parts[2].to_string()));
                    }
                }
            }
        }
    }
    if !mappings.is_empty() {
        let refs: Vec<(u8, &str)> = mappings.iter().map(|(i, n)| (*i, n.as_str())).collect();
        set_engine_structures(&refs);
    }
}

pub struct Compiler {
    options: CompilerOptions,
    lang_spec: Option<String>,
}

impl Compiler {
    pub fn new(options: CompilerOptions) -> Self {
        Self {
            options,
            lang_spec: None,
        }
    }

    pub fn set_language_spec(&mut self, spec: &str) {
        // Parse engine structure definitions immediately so the lexer
        // recognizes type names like json, effect, location etc.
        parse_engine_structure_defines(spec);
        self.lang_spec = Some(spec.to_string());
    }

    pub fn compile(
        &self,
        source: &str,
        filename: &str,
        resolver: &dyn FileResolver,
    ) -> CompileResult {
        let mut all_diagnostics = Vec::new();

        // Parse main file
        let main_parsed = self.parse_file(source, filename);
        all_diagnostics.extend(main_parsed.diagnostics);

        if main_parsed.root == NULL_NODE && !all_diagnostics.is_empty() && !self.options.collect_all_errors {
            return CompileResult {
                success: false,
                ncs: Vec::new(),
                ndb: Vec::new(),
                diagnostics: all_diagnostics,
            };
        }

        // Resolve #includes
        let mut included_files: Vec<ParsedFile> = Vec::new();
        let mut included_set: HashSet<String> = HashSet::new();
        let mut include_stack: Vec<String> = vec![filename.to_string()];

        self.resolve_includes(
            main_parsed.root,
            &main_parsed.arena,
            resolver,
            &mut included_files,
            &mut included_set,
            &mut include_stack,
            &mut all_diagnostics,
            0,
        );

        if main_parsed.root == NULL_NODE {
            return CompileResult {
                success: all_diagnostics.is_empty(),
                ncs: Vec::new(),
                ndb: Vec::new(),
                diagnostics: all_diagnostics,
            };
        }

        // Semantic analysis
        let mut checker = SemanticChecker::new(&main_parsed.arena, &main_parsed.file_names);
        checker.set_collect_all_errors(self.options.collect_all_errors);
        checker.set_require_entry_point(self.options.require_entry_point);

        if let Some(spec) = &self.lang_spec {
            checker.load_lang_spec(spec);
        }

        // Register declarations from included files first
        for inc in &included_files {
            if inc.root != NULL_NODE {
                checker.load_included_file(inc.root, &inc.arena, &inc.file_names);
            }
        }

        match checker.check(main_parsed.root) {
            Ok(()) => {
                all_diagnostics.extend(checker.diagnostics.clone());
            }
            Err(_) => {
                all_diagnostics.extend(checker.diagnostics.clone());
                if !self.options.collect_all_errors {
                    return CompileResult {
                        success: false,
                        ncs: Vec::new(),
                        ndb: Vec::new(),
                        diagnostics: all_diagnostics,
                    };
                }
            }
        }

        let has_errors = all_diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.error != CompileError::AlreadyPrinted);
        if has_errors {
            return CompileResult {
                success: false,
                ncs: Vec::new(),
                ndb: Vec::new(),
                diagnostics: all_diagnostics,
            };
        }

        // Code generation
        let mut codegen = CodeGenerator::new(&main_parsed.arena, &main_parsed.file_names);
        codegen.set_collect_all_errors(self.options.collect_all_errors);
        codegen.set_optimization_level(self.options.optimization_level);
        codegen.load_symbols(&checker);
        match codegen.generate(main_parsed.root) {
            Ok(mut ncs) => {
                all_diagnostics.extend(codegen.diagnostics);
                let has_errors = all_diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error && d.error != CompileError::AlreadyPrinted);

                // Optimization pass
                optimize::optimize_ncs(&mut ncs, self.options.optimization_level);

                // NDB debug output
                let ndb = if self.options.generate_debug {
                    let mut builder = NdbBuilder::new();
                    builder.add_file(filename);
                    builder.set_base_file(filename);
                    for f in codegen.ndb_functions.iter().cloned() {
                        builder.add_function(f);
                    }
                    for v in codegen.ndb_variables.iter().cloned() {
                        builder.add_variable(v);
                    }
                    for s in codegen.ndb_structs.iter().cloned() {
                        builder.add_struct(s);
                    }
                    for l in codegen.ndb_lines.iter().cloned() {
                        builder.add_line(l);
                    }
                    builder.generate()
                } else {
                    Vec::new()
                };

                CompileResult {
                    success: !has_errors,
                    ncs,
                    ndb,
                    diagnostics: all_diagnostics,
                }
            }
            Err(e) => {
                all_diagnostics.push(Diagnostic {
                    error: e,
                    severity: e.default_severity(),
                    file: filename.to_string(),
                    line: 0,
                    message: e.message().to_string(),
                });
                CompileResult {
                    success: false,
                    ncs: Vec::new(),
                    ndb: Vec::new(),
                    diagnostics: all_diagnostics,
                }
            }
        }
    }

    fn parse_file(&self, source: &str, filename: &str) -> ParsedFile {
        let preprocessed = preprocess_defines(source);
        let mut lexer = Lexer::new(&preprocessed, filename, 0);
        let tokens = lexer.tokenize();
        let mut diagnostics = lexer.diagnostics;

        let mut parser = Parser::new(tokens);
        parser.file_names.push(filename.to_string());
        parser.set_collect_all_errors(self.options.collect_all_errors);
        parser.set_require_entry_point(false);

        let root = match parser.parse_program() {
            Ok(r) => r,
            Err(e) => {
                diagnostics.push(Diagnostic {
                    error: e,
                    severity: e.default_severity(),
                    file: filename.to_string(),
                    line: 0,
                    message: e.message().to_string(),
                });
                NULL_NODE
            }
        };
        diagnostics.extend(parser.diagnostics.clone());

        ParsedFile {
            arena: std::mem::replace(&mut parser.arena, AstArena::new()),
            root,
            file_names: parser.file_names,
            diagnostics,
        }
    }

    fn resolve_includes(
        &self,
        node_id: NodeId,
        arena: &AstArena,
        resolver: &dyn FileResolver,
        included_files: &mut Vec<ParsedFile>,
        included_set: &mut HashSet<String>,
        include_stack: &mut Vec<String>,
        diagnostics: &mut Vec<Diagnostic>,
        depth: u32,
    ) {
        if node_id == NULL_NODE {
            return;
        }

        let node = arena.get(node_id);

        if node.op == Operation::FunctionalUnit {
            // Check if this FU contains an include
            if node.left != NULL_NODE {
                let inner = arena.get(node.left);
                if inner.op == Operation::FunctionalUnit && inner.int_data[0] == 1 {
                    // This is an include node
                    if let Some(inc_name) = &inner.string_data {
                        self.process_include(
                            inc_name,
                            inner,
                            resolver,
                            included_files,
                            included_set,
                            include_stack,
                            diagnostics,
                            depth,
                        );
                    }
                }
            }

            // Recurse into the chain
            if node.right != NULL_NODE {
                self.resolve_includes(
                    node.right,
                    arena,
                    resolver,
                    included_files,
                    included_set,
                    include_stack,
                    diagnostics,
                    depth,
                );
            }
        }
    }

    fn process_include(
        &self,
        inc_name: &str,
        node: &crate::ast::AstNode,
        resolver: &dyn FileResolver,
        included_files: &mut Vec<ParsedFile>,
        included_set: &mut HashSet<String>,
        include_stack: &mut Vec<String>,
        diagnostics: &mut Vec<Diagnostic>,
        depth: u32,
    ) {
        if depth >= self.options.max_include_depth {
            diagnostics.push(Diagnostic {
                error: CompileError::IncludeTooManyLevels,
                severity: Severity::Error,
                file: include_stack.last().cloned().unwrap_or_default(),
                line: node.line,
                message: CompileError::IncludeTooManyLevels.message().to_string(),
            });
            return;
        }

        if include_stack.contains(&inc_name.to_string()) {
            diagnostics.push(Diagnostic {
                error: CompileError::IncludeRecursive,
                severity: Severity::Error,
                file: include_stack.last().cloned().unwrap_or_default(),
                line: node.line,
                message: format!("{}: {}", CompileError::IncludeRecursive.message(), inc_name),
            });
            return;
        }

        if included_set.contains(inc_name) {
            return;
        }

        let source = match resolver.resolve(inc_name) {
            Some(s) => s,
            None => {
                diagnostics.push(Diagnostic {
                    error: CompileError::FileNotFound,
                    severity: Severity::Error,
                    file: include_stack.last().cloned().unwrap_or_default(),
                    line: node.line,
                    message: format!("{}: {}", CompileError::FileNotFound.message(), inc_name),
                });
                return;
            }
        };

        included_set.insert(inc_name.to_string());
        include_stack.push(inc_name.to_string());

        let parsed = self.parse_file(&source, &format!("{}.nss", inc_name));
        diagnostics.extend(parsed.diagnostics.clone());

        // Recursively resolve includes in the included file
        if parsed.root != NULL_NODE {
            self.resolve_includes(
                parsed.root,
                &parsed.arena,
                resolver,
                included_files,
                included_set,
                include_stack,
                diagnostics,
                depth + 1,
            );
        }

        include_stack.pop();
        included_files.push(parsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_compiler(collect_all: bool, require_entry: bool) -> Compiler {
        Compiler::new(CompilerOptions {
            require_entry_point: require_entry,
            collect_all_errors: collect_all,
            ..Default::default()
        })
    }

    #[test]
    fn test_compile_simple() {
        let c = make_compiler(true, false);
        let r = MapResolver::new();
        let result = c.compile("void foo() { }", "test.nss", &r);
        assert!(
            result.diagnostics.is_empty(),
            "Unexpected errors: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn test_compile_with_error() {
        let c = make_compiler(true, false);
        let r = MapResolver::new();
        let result = c.compile("void foo() { int x = ; }", "test.nss", &r);
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn test_include_resolution() {
        let c = make_compiler(false, true);
        let mut r = MapResolver::new();
        r.add_file("lib", "int helper(int n) { return n * 2; }");
        let result = c.compile(
            "#include \"lib\"\nvoid main() { int x = helper(3); }",
            "main.nss",
            &r,
        );
        assert!(
            result.diagnostics.is_empty(),
            "Unexpected errors: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn test_include_file_not_found() {
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
    fn test_recursive_include() {
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
    fn test_chained_includes() {
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
        assert!(
            result.diagnostics.is_empty(),
            "Unexpected errors: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn test_cross_file_error_reporting() {
        let c = make_compiler(true, true);
        let mut r = MapResolver::new();
        r.add_file("lib", "int helper(int n) { return n * 2; }");
        let result = c.compile(
            "#include \"lib\"\nvoid main() { int x = helper(\"wrong_type\"); }",
            "main.nss",
            &r,
        );
        assert!(
            result.diagnostics.iter().any(|d| d.error == CompileError::MismatchedTypes),
            "Expected type mismatch from calling included function with wrong arg type: {:?}",
            result.diagnostics
        );
    }
}
