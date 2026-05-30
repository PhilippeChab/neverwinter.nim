use std::fmt::Write;

/// Type abbreviations used in NDB output, matching GenerateDebuggerTypeAbbreviation
/// in the C++ compiler (scriptcompfinalcode.cpp:6747-6803).
/// `struct_index_of` resolves a struct name to its index in the struct table, used to
/// emit Struct (and the vector pseudo-struct) as `tNNNN`.
fn type_abbrev(
    nw_type: crate::types::NwType,
    struct_name: &str,
    struct_index_of: &dyn Fn(&str) -> Option<usize>,
) -> String {
    use crate::types::NwType;
    match nw_type {
        NwType::Void => "v".to_string(),
        NwType::Integer => "i".to_string(),
        NwType::Float => "f".to_string(),
        NwType::String => "s".to_string(),
        NwType::Object => "o".to_string(),
        // C++ models vector as a struct; emit it via the struct table.
        NwType::Vector => match struct_index_of("vector") {
            Some(i) => format!("t{:04}", i),
            None => "t0000".to_string(),
        },
        // C++ falls through to "?".
        NwType::Action => "?".to_string(),
        NwType::EngineStructure(n) => format!("e{}", n),
        NwType::Struct => match struct_index_of(struct_name) {
            Some(i) => format!("t{:04}", i),
            None => format!("t0000"),
        },
    }
}

#[derive(Debug, Clone)]
pub struct NdbFunctionEntry {
    pub name: String,
    pub return_type: crate::types::NwType,
    pub return_struct_name: String,
    pub code_start: u32,
    pub code_end: u32,
    pub params: Vec<(crate::types::NwType, String)>,
}

#[derive(Debug, Clone)]
pub struct NdbVarEntry {
    pub name: String,
    pub var_type: crate::types::NwType,
    pub struct_name: String,
    pub stack_loc: u32,
    pub code_start: u32,
    pub code_end: u32,
}

#[derive(Debug, Clone)]
pub struct NdbStructDef {
    pub name: String,
    pub fields: Vec<(String, crate::types::NwType, String)>, // (name, type, struct_name)
}

#[derive(Debug, Clone)]
pub struct NdbLineEntry {
    pub file_id: u8,
    pub line: u32,
    pub code_start: u32,
    pub code_end: u32,
}

#[derive(Debug, Default)]
pub struct NdbBuilder {
    pub files: Vec<String>,
    pub base_file: Option<String>,
    pub structs: Vec<NdbStructDef>,
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

    pub fn set_base_file(&mut self, name: &str) {
        self.base_file = Some(name.to_string());
    }

    pub fn add_function(&mut self, entry: NdbFunctionEntry) {
        self.functions.push(entry);
    }

    pub fn add_variable(&mut self, entry: NdbVarEntry) {
        self.variables.push(entry);
    }

    pub fn add_struct(&mut self, def: NdbStructDef) {
        self.structs.push(def);
    }

    pub fn add_line(&mut self, entry: NdbLineEntry) {
        self.line_entries.push(entry);
    }

    pub fn generate(&self) -> Vec<u8> {
        let mut out = String::new();

        // Header
        writeln!(out, "NDB V1.0").unwrap();

        // Counts line: file_count, struct_count, function_count, var_count, line_count
        // Format: "%07d %07d %07d %07d %07d\n" (40 chars)
        writeln!(
            out,
            "{:07} {:07} {:07} {:07} {:07}",
            self.files.len(),
            self.structs.len(),
            self.functions.len(),
            self.variables.len(),
            self.line_entries.len(),
        ).unwrap();

        // File entries: "N%02d %s\n" for base, "n%02d %s\n" for others
        for (i, name) in self.files.iter().enumerate() {
            let prefix = if Some(name) == self.base_file.as_ref() { 'N' } else { 'n' };
            writeln!(out, "{}{:02} {}", prefix, i, name).unwrap();
        }

        // Build a name→index lookup so type_abbrev can emit struct/vector as tNNNN.
        let struct_lookup = |n: &str| self.structs.iter().position(|s| s.name == n);

        // Struct entries: "s %02d %s\n" then "sf <type> <name>\n" per field
        for s in &self.structs {
            writeln!(out, "s {:02} {}", s.fields.len(), s.name).unwrap();
            for (fname, ftype, fstruct) in &s.fields {
                writeln!(out, "sf {} {}", type_abbrev(*ftype, fstruct, &struct_lookup), fname).unwrap();
            }
        }

        // Function entries: "f %08x %08x %03d <type> <name>\n" + "fp <type>\n" per param
        for f in &self.functions {
            writeln!(
                out,
                "f {:08x} {:08x} {:03} {} {}",
                f.code_start,
                f.code_end,
                f.params.len(),
                type_abbrev(f.return_type, &f.return_struct_name, &struct_lookup),
                f.name,
            ).unwrap();
            for (ptype, pstruct) in &f.params {
                writeln!(out, "fp {}", type_abbrev(*ptype, pstruct, &struct_lookup)).unwrap();
            }
        }

        // Variable entries: "v %08x %08x %08x <type> <name>\n"
        for v in &self.variables {
            writeln!(
                out,
                "v {:08x} {:08x} {:08x} {} {}",
                v.code_start,
                v.code_end,
                v.stack_loc,
                type_abbrev(v.var_type, &v.struct_name, &struct_lookup),
                v.name,
            ).unwrap();
        }

        // C++ scriptcompfinalcode.cpp:6980 — "l%02d %07d %08x %08x\n"
        // (no space after l; 7-digit decimal line number)
        for l in &self.line_entries {
            writeln!(
                out,
                "l{:02} {:07} {:08x} {:08x}",
                l.file_id, l.line, l.code_start, l.code_end,
            ).unwrap();
        }

        out.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NwType;

    #[test]
    fn test_empty_ndb() {
        let b = NdbBuilder::new();
        let s = String::from_utf8(b.generate()).unwrap();
        assert!(s.starts_with("NDB V1.0\n"));
        assert!(s.contains("0000000 0000000 0000000 0000000 0000000\n"));
    }

    #[test]
    fn test_ndb_with_base_file() {
        let mut b = NdbBuilder::new();
        b.add_file("test.nss");
        b.set_base_file("test.nss");
        let s = String::from_utf8(b.generate()).unwrap();
        assert!(s.contains("N00 test.nss"));
    }

    #[test]
    fn test_ndb_with_included_file() {
        let mut b = NdbBuilder::new();
        b.add_file("main.nss");
        b.add_file("lib.nss");
        b.set_base_file("main.nss");
        let s = String::from_utf8(b.generate()).unwrap();
        assert!(s.contains("N00 main.nss"));
        assert!(s.contains("n01 lib.nss"));
    }

    #[test]
    fn test_ndb_function_entry() {
        let mut b = NdbBuilder::new();
        b.add_function(NdbFunctionEntry {
            name: "main".to_string(),
            return_type: NwType::Void,
            return_struct_name: String::new(),
            code_start: 0x0d,
            code_end: 0x19,
            params: vec![],
        });
        let s = String::from_utf8(b.generate()).unwrap();
        assert!(s.contains("f 0000000d 00000019 000 v main"));
    }

    #[test]
    fn test_ndb_variable_entry() {
        let mut b = NdbBuilder::new();
        b.add_variable(NdbVarEntry {
            name: "x".to_string(),
            var_type: NwType::Integer,
            struct_name: String::new(),
            stack_loc: 0x10,
            code_start: 0x0f,
            code_end: 0x19,
        });
        let s = String::from_utf8(b.generate()).unwrap();
        assert!(s.contains("v 0000000f 00000019 00000010 i x"));
    }

    #[test]
    fn test_ndb_line_entry() {
        let mut b = NdbBuilder::new();
        b.add_line(NdbLineEntry {
            file_id: 0,
            line: 5,
            code_start: 0x0d,
            code_end: 0x13,
        });
        let s = String::from_utf8(b.generate()).unwrap();
        // C++ format: "l%02d %07d %08x %08x"
        assert!(s.contains("l00 0000005 0000000d 00000013"));
    }
}
