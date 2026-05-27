use std::collections::HashSet;

use crate::ast::{AstArena, NodeId, NULL_NODE, Operation};
use crate::codegen::CodeGenerator;
use crate::errors::{CompileError, Diagnostic};
use crate::lexer::Lexer;
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

        if all_diagnostics
            .iter()
            .any(|d| d.error != CompileError::AlreadyPrinted)
        {
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
        codegen.load_symbols(&checker);
        match codegen.generate(main_parsed.root) {
            Ok(ncs) => {
                all_diagnostics.extend(codegen.diagnostics);
                let has_errors = all_diagnostics
                    .iter()
                    .any(|d| d.error != CompileError::AlreadyPrinted);
                CompileResult {
                    success: !has_errors,
                    ncs,
                    ndb: Vec::new(),
                    diagnostics: all_diagnostics,
                }
            }
            Err(e) => {
                all_diagnostics.push(Diagnostic {
                    error: e,
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
        let mut lexer = Lexer::new(source, filename, 0);
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
                file: include_stack.last().cloned().unwrap_or_default(),
                line: node.line,
                message: CompileError::IncludeTooManyLevels.message().to_string(),
            });
            return;
        }

        if include_stack.contains(&inc_name.to_string()) {
            diagnostics.push(Diagnostic {
                error: CompileError::IncludeRecursive,
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
