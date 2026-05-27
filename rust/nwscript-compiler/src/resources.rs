use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const RESTYPE_NSS: u16 = 2009;
pub const RESTYPE_NCS: u16 = 2010;
pub const RESTYPE_NDB: u16 = 2064;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResRef {
    pub name: String,
    pub res_type: u16,
}

impl ResRef {
    pub fn nss(name: &str) -> Self {
        Self {
            name: name.to_lowercase(),
            res_type: RESTYPE_NSS,
        }
    }
}

#[derive(Debug)]
struct KeyEntry {
    name: String,
    res_type: u16,
    res_id: u32,
}

impl KeyEntry {
    fn bif_index(&self) -> usize {
        (self.res_id >> 20) as usize
    }

    fn variable_index(&self) -> usize {
        (self.res_id & 0x000F_FFFF) as usize
    }
}

#[derive(Debug)]
struct BifFileRef {
    filename: String,
    file_size: u32,
}

#[derive(Debug)]
struct BifVarEntry {
    offset: u32,
    size: u32,
    res_type: u32,
}

pub struct KeyTable {
    bif_files: Vec<BifFileRef>,
    entries: HashMap<ResRef, KeyEntry>,
    base_path: PathBuf,
}

impl KeyTable {
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let data = fs::read(path)?;
        let base_path = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Self::parse(&data, base_path)
    }

    fn parse(data: &[u8], base_path: PathBuf) -> io::Result<Self> {
        if data.len() < 64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "KEY file too short"));
        }

        let magic = &data[0..4];
        if magic != b"KEY " {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid KEY magic: {:?}", magic),
            ));
        }

        let version = &data[4..8];
        let is_e1 = version == b"E1  ";

        let bif_count = read_u32_le(data, 8) as usize;
        let key_count = read_u32_le(data, 12) as usize;
        let file_table_offset = read_u32_le(data, 16) as usize;
        let key_table_offset = read_u32_le(data, 20) as usize;

        // Read BIF file table
        let mut bif_files = Vec::with_capacity(bif_count);
        for i in 0..bif_count {
            let entry_offset = file_table_offset + i * 12;
            if entry_offset + 12 > data.len() {
                break;
            }
            let file_size = read_u32_le(data, entry_offset);
            let filename_offset = read_u32_le(data, entry_offset + 4) as usize;
            let filename_size = read_u16_le(data, entry_offset + 8) as usize;

            let filename = if filename_offset + filename_size <= data.len() {
                String::from_utf8_lossy(&data[filename_offset..filename_offset + filename_size])
                    .trim_end_matches('\0')
                    .replace('\\', "/")
                    .to_string()
            } else {
                String::new()
            };

            bif_files.push(BifFileRef {
                filename,
                file_size,
            });
        }

        // Read KEY table
        let key_entry_size: usize = if is_e1 { 42 } else { 22 };
        let mut entries = HashMap::with_capacity(key_count);

        for i in 0..key_count {
            let entry_offset = key_table_offset + i * key_entry_size;
            if entry_offset + key_entry_size > data.len() {
                break;
            }

            let name_bytes = &data[entry_offset..entry_offset + 16];
            let name = String::from_utf8_lossy(name_bytes)
                .trim_end_matches('\0')
                .to_lowercase();

            let res_type = read_u16_le(data, entry_offset + 16);
            let res_id = read_u32_le(data, entry_offset + 18);

            let resref = ResRef {
                name: name.clone(),
                res_type,
            };

            entries.insert(
                resref,
                KeyEntry {
                    name,
                    res_type,
                    res_id,
                },
            );
        }

        Ok(KeyTable {
            bif_files,
            entries,
            base_path,
        })
    }

    pub fn contains(&self, resref: &ResRef) -> bool {
        self.entries.contains_key(resref)
    }

    pub fn read_resource(&self, resref: &ResRef) -> io::Result<Vec<u8>> {
        let entry = self.entries.get(resref).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("Resource not found: {}", resref.name))
        })?;

        let bif_idx = entry.bif_index();
        if bif_idx >= self.bif_files.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("BIF index {} out of range", bif_idx),
            ));
        }

        let bif_ref = &self.bif_files[bif_idx];
        let bif_path = self.base_path.join(&bif_ref.filename);

        let bif_data = fs::read(&bif_path).map_err(|e| {
            io::Error::new(e.kind(), format!("Failed to read BIF {}: {}", bif_path.display(), e))
        })?;

        read_from_bif(&bif_data, entry.variable_index())
    }

    pub fn list_resources(&self, res_type: u16) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(r, _)| r.res_type == res_type)
            .map(|(r, _)| r.name.clone())
            .collect()
    }
}

fn read_from_bif(data: &[u8], var_index: usize) -> io::Result<Vec<u8>> {
    if data.len() < 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "BIF file too short"));
    }

    let magic = &data[0..4];
    if magic != b"BIFF" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid BIF magic: {:?}", magic),
        ));
    }

    let version = &data[4..8];
    let is_e1 = version == b"E1  ";

    let var_count = read_u32_le(data, 8) as usize;
    let var_table_offset = if is_e1 {
        read_u32_le(data, 16) as usize
    } else {
        read_u32_le(data, 16) as usize
    };

    if var_index >= var_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Variable index {} >= count {}", var_index, var_count),
        ));
    }

    let entry_size: usize = if is_e1 { 32 } else { 16 };
    let entry_offset = var_table_offset + var_index * entry_size;

    if entry_offset + entry_size > data.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "BIF entry out of bounds"));
    }

    let res_offset = read_u32_le(data, entry_offset + 4) as usize;
    let res_size = read_u32_le(data, entry_offset + 8) as usize;

    if res_offset + res_size > data.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "BIF resource data out of bounds"));
    }

    Ok(data[res_offset..res_offset + res_size].to_vec())
}

/// Scans a directory recursively for .nss files and builds a name → path map.
pub fn scan_nss_directory(dir: &Path) -> io::Result<HashMap<String, PathBuf>> {
    let mut map = HashMap::new();
    if !dir.exists() {
        return Ok(map);
    }
    scan_dir_recursive(dir, &mut map)?;
    Ok(map)
}

fn scan_dir_recursive(dir: &Path, map: &mut HashMap<String, PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            scan_dir_recursive(&path, map)?;
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("nss") {
                if let Some(stem) = path.file_stem() {
                    let name = stem.to_string_lossy().to_lowercase();
                    map.insert(name, path);
                }
            }
        }
    }
    Ok(())
}

/// A FileResolver that reads from KEY/BIF archives and loose .nss files.
pub struct NwnResolver {
    key_tables: Vec<KeyTable>,
    loose_files: HashMap<String, PathBuf>,
}

impl NwnResolver {
    pub fn new() -> Self {
        Self {
            key_tables: Vec::new(),
            loose_files: HashMap::new(),
        }
    }

    pub fn add_key_table(&mut self, key: KeyTable) {
        self.key_tables.push(key);
    }

    pub fn add_directory(&mut self, dir: &Path) -> io::Result<()> {
        let files = scan_nss_directory(dir)?;
        self.loose_files.extend(files);
        Ok(())
    }

    pub fn add_loose_file(&mut self, name: &str, path: PathBuf) {
        self.loose_files.insert(name.to_lowercase(), path);
    }
}

impl Default for NwnResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::compiler::FileResolver for NwnResolver {
    fn resolve(&self, filename: &str) -> Option<String> {
        let name = filename.to_lowercase();

        // Check loose files first (higher priority)
        if let Some(path) = self.loose_files.get(&name) {
            return fs::read_to_string(path).ok();
        }

        // Check KEY/BIF archives
        let resref = ResRef::nss(&name);
        for key in &self.key_tables {
            if key.contains(&resref) {
                if let Ok(data) = key.read_resource(&resref) {
                    return String::from_utf8(data).ok();
                }
            }
        }

        None
    }
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::FileResolver;

    #[test]
    fn test_resref_nss() {
        let r = ResRef::nss("MyScript");
        assert_eq!(r.name, "myscript");
        assert_eq!(r.res_type, RESTYPE_NSS);
    }

    #[test]
    fn test_key_entry_bif_index() {
        let entry = KeyEntry {
            name: "test".to_string(),
            res_type: RESTYPE_NSS,
            res_id: 0x0010_0005, // BIF index 1, var index 5
        };
        assert_eq!(entry.bif_index(), 1);
        assert_eq!(entry.variable_index(), 5);
    }

    #[test]
    fn test_key_entry_max_values() {
        let entry = KeyEntry {
            name: "test".to_string(),
            res_type: RESTYPE_NSS,
            res_id: 0xFFF0_0000, // BIF index 4095, var index 0
        };
        assert_eq!(entry.bif_index(), 4095);
        assert_eq!(entry.variable_index(), 0);
    }

    #[test]
    fn test_scan_nss_empty_dir() {
        let map = scan_nss_directory(Path::new("/nonexistent")).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn test_nwn_resolver_loose_files() {
        let mut resolver = NwnResolver::new();
        let tmp = std::env::temp_dir().join("nwscript_test_loose.nss");
        fs::write(&tmp, "int helper() { return 1; }").unwrap();
        resolver.add_loose_file("nwscript_test_loose", tmp.clone());

        let result = resolver.resolve("nwscript_test_loose");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "int helper() { return 1; }");

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_nwn_resolver_not_found() {
        let resolver = NwnResolver::new();
        assert!(resolver.resolve("nonexistent").is_none());
    }

    fn make_test_key_bif() -> (Vec<u8>, Vec<u8>) {
        // Build a minimal BIF with one resource
        let resource_data = b"int test_fn() { return 42; }";
        let var_table_offset: u32 = 20;
        let mut bif = Vec::new();
        bif.extend_from_slice(b"BIFF"); // magic
        bif.extend_from_slice(b"V1  "); // version
        bif.extend_from_slice(&1u32.to_le_bytes()); // var count = 1
        bif.extend_from_slice(&0u32.to_le_bytes()); // fixed count = 0
        bif.extend_from_slice(&var_table_offset.to_le_bytes()); // var table offset

        // Variable resource entry (16 bytes)
        let data_offset = var_table_offset + 16; // right after the var table
        bif.extend_from_slice(&0u32.to_le_bytes()); // res ID
        bif.extend_from_slice(&data_offset.to_le_bytes()); // offset
        bif.extend_from_slice(&(resource_data.len() as u32).to_le_bytes()); // size
        bif.extend_from_slice(&(RESTYPE_NSS as u32).to_le_bytes()); // type

        // Resource data
        bif.extend_from_slice(resource_data);

        // Build a minimal KEY referencing this BIF
        let bif_filename = b"data/test.bif\0";
        let file_table_offset: u32 = 64;
        let filename_offset: u32 = file_table_offset + 12;
        let key_table_offset: u32 = filename_offset + bif_filename.len() as u32;

        let mut key = Vec::new();
        key.extend_from_slice(b"KEY "); // magic
        key.extend_from_slice(b"V1  "); // version
        key.extend_from_slice(&1u32.to_le_bytes()); // bif count
        key.extend_from_slice(&1u32.to_le_bytes()); // key count
        key.extend_from_slice(&file_table_offset.to_le_bytes());
        key.extend_from_slice(&key_table_offset.to_le_bytes());
        key.extend_from_slice(&124u32.to_le_bytes()); // build year
        key.extend_from_slice(&1u32.to_le_bytes()); // build day
        key.extend_from_slice(&[0u8; 32]); // reserved

        // File table entry (12 bytes)
        key.extend_from_slice(&(bif.len() as u32).to_le_bytes()); // file size
        key.extend_from_slice(&filename_offset.to_le_bytes()); // filename offset
        key.extend_from_slice(&(bif_filename.len() as u16 - 1).to_le_bytes()); // filename size (minus null)
        key.extend_from_slice(&1u16.to_le_bytes()); // drive count

        // Filename
        key.extend_from_slice(bif_filename);

        // Key table entry (22 bytes)
        let mut name_padded = [0u8; 16];
        name_padded[..7].copy_from_slice(b"test_fn");
        key.extend_from_slice(&name_padded); // name
        key.extend_from_slice(&RESTYPE_NSS.to_le_bytes()); // type
        let res_id: u32 = 0x0000_0000; // BIF 0, var index 0
        key.extend_from_slice(&res_id.to_le_bytes());

        (key, bif)
    }

    #[test]
    fn test_parse_key_file() {
        let (key_data, _bif_data) = make_test_key_bif();
        let key = KeyTable::parse(&key_data, PathBuf::from(".")).unwrap();

        assert_eq!(key.bif_files.len(), 1);
        assert!(key.bif_files[0].filename.contains("test.bif"));
        assert!(key.contains(&ResRef::nss("test_fn")));
        assert!(!key.contains(&ResRef::nss("nonexistent")));
    }

    #[test]
    fn test_read_from_bif() {
        let (_key_data, bif_data) = make_test_key_bif();
        let result = read_from_bif(&bif_data, 0).unwrap();
        assert_eq!(
            String::from_utf8(result).unwrap(),
            "int test_fn() { return 42; }"
        );
    }

    #[test]
    fn test_list_nss_resources() {
        let (key_data, _) = make_test_key_bif();
        let key = KeyTable::parse(&key_data, PathBuf::from(".")).unwrap();
        let nss_list = key.list_resources(RESTYPE_NSS);
        assert_eq!(nss_list.len(), 1);
        assert_eq!(nss_list[0], "test_fn");
    }
}
