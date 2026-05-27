use std::fmt::Write;

#[derive(Debug, Clone)]
pub struct NdbLineEntry {
    pub file_id: u32,
    pub line: u32,
    pub code_start: u32,
    pub code_end: u32,
}

#[derive(Debug, Clone)]
pub struct NdbFunctionEntry {
    pub name: String,
    pub code_start: u32,
    pub code_end: u32,
    pub return_type: String,
}

#[derive(Debug, Clone)]
pub struct NdbVarEntry {
    pub name: String,
    pub var_type: String,
    pub stack_offset: i32,
    pub code_start: u32,
    pub code_end: u32,
}

#[derive(Debug, Default)]
pub struct NdbBuilder {
    pub files: Vec<String>,
    pub functions: Vec<NdbFunctionEntry>,
    pub variables: Vec<NdbVarEntry>,
    pub line_entries: Vec<NdbLineEntry>,
}

impl NdbBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, name: &str) -> u32 {
        let id = self.files.len() as u32;
        self.files.push(name.to_string());
        id
    }

    pub fn add_function(&mut self, entry: NdbFunctionEntry) {
        self.functions.push(entry);
    }

    pub fn add_variable(&mut self, entry: NdbVarEntry) {
        self.variables.push(entry);
    }

    pub fn add_line(&mut self, entry: NdbLineEntry) {
        self.line_entries.push(entry);
    }

    pub fn generate(&self) -> Vec<u8> {
        let mut out = String::new();

        // NDB format: text-based debug info
        // Line 1: NDB version
        writeln!(out, "NDB V1.0").unwrap();

        // File list
        writeln!(out, "N {}", self.files.len()).unwrap();
        for f in &self.files {
            writeln!(out, "F {}", f).unwrap();
        }

        // Function list
        writeln!(out, "n {}", self.functions.len()).unwrap();
        for f in &self.functions {
            writeln!(
                out,
                "f {} {} {} {}",
                f.name, f.return_type, f.code_start, f.code_end
            ).unwrap();
        }

        // Variable list
        writeln!(out, "v {}", self.variables.len()).unwrap();
        for v in &self.variables {
            writeln!(
                out,
                "V {} {} {} {} {}",
                v.name, v.var_type, v.stack_offset, v.code_start, v.code_end
            ).unwrap();
        }

        // Line number mappings
        writeln!(out, "l {}", self.line_entries.len()).unwrap();
        for l in &self.line_entries {
            writeln!(
                out,
                "L {} {} {} {}",
                l.file_id, l.line, l.code_start, l.code_end
            ).unwrap();
        }

        out.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_ndb() {
        let builder = NdbBuilder::new();
        let ndb = builder.generate();
        let text = String::from_utf8(ndb).unwrap();
        assert!(text.starts_with("NDB V1.0"));
        assert!(text.contains("N 0"));
        assert!(text.contains("n 0"));
    }

    #[test]
    fn test_ndb_with_file() {
        let mut builder = NdbBuilder::new();
        builder.add_file("test.nss");
        let ndb = builder.generate();
        let text = String::from_utf8(ndb).unwrap();
        assert!(text.contains("N 1"));
        assert!(text.contains("F test.nss"));
    }

    #[test]
    fn test_ndb_with_function() {
        let mut builder = NdbBuilder::new();
        builder.add_file("test.nss");
        builder.add_function(NdbFunctionEntry {
            name: "main".to_string(),
            code_start: 13,
            code_end: 25,
            return_type: "v".to_string(),
        });
        let ndb = builder.generate();
        let text = String::from_utf8(ndb).unwrap();
        assert!(text.contains("f main v 13 25"));
    }

    #[test]
    fn test_ndb_with_variable() {
        let mut builder = NdbBuilder::new();
        builder.add_variable(NdbVarEntry {
            name: "x".to_string(),
            var_type: "i".to_string(),
            stack_offset: -4,
            code_start: 15,
            code_end: 25,
        });
        let ndb = builder.generate();
        let text = String::from_utf8(ndb).unwrap();
        assert!(text.contains("V x i -4 15 25"));
    }

    #[test]
    fn test_ndb_with_line_entry() {
        let mut builder = NdbBuilder::new();
        builder.add_line(NdbLineEntry {
            file_id: 0,
            line: 5,
            code_start: 13,
            code_end: 19,
        });
        let ndb = builder.generate();
        let text = String::from_utf8(ndb).unwrap();
        assert!(text.contains("L 0 5 13 19"));
    }
}
