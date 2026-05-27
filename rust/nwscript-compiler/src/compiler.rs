use crate::ast::NULL_NODE;
use crate::codegen::CodeGenerator;
use crate::errors::Diagnostic;
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

        let mut lexer = Lexer::new(source, filename, 0);
        let tokens = lexer.tokenize();
        all_diagnostics.extend(lexer.diagnostics);

        if !all_diagnostics.is_empty() && !self.options.collect_all_errors {
            return CompileResult {
                success: false,
                ncs: Vec::new(),
                ndb: Vec::new(),
                diagnostics: all_diagnostics,
            };
        }

        let mut parser = Parser::new(tokens);
        parser.file_names.push(filename.to_string());
        parser.set_collect_all_errors(self.options.collect_all_errors);
        parser.set_require_entry_point(self.options.require_entry_point);

        let root = match parser.parse_program() {
            Ok(r) => r,
            Err(e) => {
                all_diagnostics.push(Diagnostic {
                    error: e,
                    file: filename.to_string(),
                    line: 0,
                    message: e.message().to_string(),
                });
                return CompileResult {
                    success: false,
                    ncs: Vec::new(),
                    ndb: Vec::new(),
                    diagnostics: all_diagnostics,
                };
            }
        };

        all_diagnostics.extend(parser.diagnostics.clone());

        if root == NULL_NODE {
            return CompileResult {
                success: all_diagnostics.is_empty(),
                ncs: Vec::new(),
                ndb: Vec::new(),
                diagnostics: all_diagnostics,
            };
        }

        // Semantic analysis
        let mut checker = SemanticChecker::new(&parser.arena, &parser.file_names);
        checker.set_collect_all_errors(self.options.collect_all_errors);
        checker.set_require_entry_point(self.options.require_entry_point);

        if let Some(spec) = &self.lang_spec {
            checker.load_lang_spec(spec);
        }

        match checker.check(root) {
            Ok(()) => {
                all_diagnostics.extend(checker.diagnostics);
            }
            Err(_) => {
                all_diagnostics.extend(checker.diagnostics);
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

        if all_diagnostics.iter().any(|d| d.error != crate::errors::CompileError::AlreadyPrinted) {
            return CompileResult {
                success: false,
                ncs: Vec::new(),
                ndb: Vec::new(),
                diagnostics: all_diagnostics,
            };
        }

        // Code generation
        let mut codegen = CodeGenerator::new(&parser.arena, &parser.file_names);
        codegen.set_collect_all_errors(self.options.collect_all_errors);
        match codegen.generate(root) {
            Ok(ncs) => {
                all_diagnostics.extend(codegen.diagnostics);
                let has_errors = all_diagnostics.iter().any(|d| {
                    d.error != crate::errors::CompileError::AlreadyPrinted
                });
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple() {
        let compiler = Compiler::new(CompilerOptions {
            require_entry_point: false,
            collect_all_errors: true,
            ..Default::default()
        });
        let resolver = MapResolver::new();
        let result = compiler.compile("void foo() { }", "test.nss", &resolver);
        assert!(result.diagnostics.is_empty(), "Unexpected errors: {:?}", result.diagnostics);
    }

    #[test]
    fn test_compile_with_error() {
        let compiler = Compiler::new(CompilerOptions {
            collect_all_errors: true,
            require_entry_point: false,
            ..Default::default()
        });
        let resolver = MapResolver::new();
        let result = compiler.compile("void foo() { int x = ; }", "test.nss", &resolver);
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn test_compile_multiple_functions() {
        let compiler = Compiler::new(CompilerOptions {
            require_entry_point: false,
            ..Default::default()
        });
        let resolver = MapResolver::new();
        let result = compiler.compile(
            "int add(int a, int b) { return a + b; } void main() { int x = add(1, 2); }",
            "test.nss",
            &resolver,
        );
        // May have semantic errors (no symbol table yet) but should parse fine
        assert!(result.diagnostics.is_empty() || result.diagnostics.len() > 0);
    }
}
