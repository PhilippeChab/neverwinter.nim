#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NwType {
    Void,
    Integer,
    Float,
    String,
    Object,
    Vector,
    Action,
    EngineStructure(u8),
    Struct,
}

impl NwType {
    pub fn size_bytes(self) -> i32 {
        match self {
            Self::Void => 0,
            Self::Integer => 4,
            Self::Float => 4,
            Self::String => 4,
            Self::Object => 4,
            Self::Vector => 12,
            Self::Action => 4,
            Self::EngineStructure(_) => 4,
            Self::Struct => 0,
        }
    }

    pub fn auxcode(self) -> u8 {
        match self {
            Self::Void => 0x01,
            Self::Integer => 0x03,
            Self::Float => 0x04,
            Self::String => 0x05,
            Self::Object => 0x06,
            Self::Action => 0x02,
            Self::EngineStructure(n) => 0x10 + n,
            Self::Vector => 0x03, // vectors use int auxcode at instruction level
            Self::Struct => 0x03,
        }
    }

    pub fn auxcode_pair(self, other: NwType) -> Option<u8> {
        Some(match (self, other) {
            (Self::Integer, Self::Integer) => 0x20,
            (Self::Float, Self::Float) => 0x21,
            (Self::Object, Self::Object) => 0x22,
            (Self::String, Self::String) => 0x23,
            (Self::Struct, Self::Struct) => 0x24,
            (Self::Integer, Self::Float) => 0x25,
            (Self::Float, Self::Integer) => 0x26,
            (Self::EngineStructure(a), Self::EngineStructure(b)) if a == b => 0x30 + a,
            (Self::Vector, Self::Vector) => 0x3a,
            (Self::Vector, Self::Float) => 0x3b,
            (Self::Float, Self::Vector) => 0x3c,
            _ => return None,
        })
    }
}
