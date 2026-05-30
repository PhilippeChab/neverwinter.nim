#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenType {
    Unknown,
    // Literals
    Integer,
    HexInteger,
    BinaryInteger,
    OctalInteger,
    Float,
    String,
    RawString,
    HashedString,
    // Identifiers
    Identifier,
    Variable,
    IntegerIdentifier,
    FloatIdentifier,
    StringIdentifier,
    ObjectIdentifier,
    VoidIdentifier,
    VectorIdentifier,
    StructureIdentifier,
    EngineStructure0Identifier,
    EngineStructure1Identifier,
    EngineStructure2Identifier,
    EngineStructure3Identifier,
    EngineStructure4Identifier,
    EngineStructure5Identifier,
    EngineStructure6Identifier,
    EngineStructure7Identifier,
    EngineStructure8Identifier,
    EngineStructure9Identifier,
    // Operators
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulus,
    LogicalAnd,
    LogicalOr,
    BooleanAnd,
    BooleanNot,
    InclusiveOr,
    ExclusiveOr,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
    Tilde,
    // Comparison
    CondGreaterEqual,
    CondLessEqual,
    CondGreaterThan,
    CondLessThan,
    CondNotEqual,
    CondEqual,
    // Assignment
    AssignmentEqual,
    AssignmentMinus,
    AssignmentPlus,
    AssignmentMultiply,
    AssignmentDivide,
    AssignmentModulus,
    AssignmentAnd,
    AssignmentXor,
    AssignmentOr,
    AssignmentShiftLeft,
    AssignmentShiftRight,
    AssignmentUShiftRight,
    // Inc/Dec
    Increment,
    Decrement,
    // Delimiters
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    LeftSquareBracket,
    RightSquareBracket,
    Semicolon,
    Comma,
    QuestionMark,
    Colon,
    StructurePartSpecify,
    // Keywords - types
    KeywordInt,
    KeywordFloat,
    KeywordString,
    KeywordObject,
    KeywordVoid,
    KeywordVector,
    KeywordStruct,
    KeywordAction,
    KeywordConst,
    // Keywords - control flow
    KeywordIf,
    KeywordElse,
    KeywordWhile,
    KeywordFor,
    KeywordDo,
    KeywordSwitch,
    KeywordCase,
    KeywordDefault,
    KeywordBreak,
    KeywordContinue,
    KeywordReturn,
    // Keywords - literals
    KeywordObjectSelf,
    KeywordObjectInvalid,
    KeywordJsonNull,
    KeywordJsonFalse,
    KeywordJsonTrue,
    KeywordJsonObject,
    KeywordJsonArray,
    KeywordJsonString,
    KeywordLocationInvalid,
    // Keywords - preprocessor / macros
    KeywordInclude,
    KeywordDefine,
    KeywordEngineNumStructuresDefinition,
    KeywordEngineStructureDefinition,
    KeywordEngineStructure0,
    KeywordEngineStructure1,
    KeywordEngineStructure2,
    KeywordEngineStructure3,
    KeywordEngineStructure4,
    KeywordEngineStructure5,
    KeywordEngineStructure6,
    KeywordEngineStructure7,
    KeywordEngineStructure8,
    KeywordEngineStructure9,
    // Keywords - built-in macros
    KeywordDashDashFunction,
    KeywordDashDashFile,
    KeywordDashDashLine,
    KeywordDashDashDate,
    KeywordDashDashTime,
    // Comments (internal lexer state)
    CplusComment,
    CComment,
    // Special
    Eof,
}

impl TokenType {
    pub fn is_type_specifier(self) -> bool {
        matches!(
            self,
            Self::KeywordInt
                | Self::KeywordFloat
                | Self::KeywordString
                | Self::KeywordObject
                | Self::KeywordVoid
                | Self::KeywordVector
                | Self::KeywordAction
                | Self::KeywordStruct
                | Self::KeywordEngineStructure0
                | Self::KeywordEngineStructure1
                | Self::KeywordEngineStructure2
                | Self::KeywordEngineStructure3
                | Self::KeywordEngineStructure4
                | Self::KeywordEngineStructure5
                | Self::KeywordEngineStructure6
                | Self::KeywordEngineStructure7
                | Self::KeywordEngineStructure8
                | Self::KeywordEngineStructure9
        )
    }

    pub fn is_non_void_type_specifier(self) -> bool {
        self.is_type_specifier() && self != Self::KeywordVoid && self != Self::KeywordAction
    }

    pub fn is_assignment_operator(self) -> bool {
        matches!(
            self,
            Self::AssignmentEqual
                | Self::AssignmentMinus
                | Self::AssignmentPlus
                | Self::AssignmentMultiply
                | Self::AssignmentDivide
                | Self::AssignmentModulus
                | Self::AssignmentAnd
                | Self::AssignmentXor
                | Self::AssignmentOr
                | Self::AssignmentShiftLeft
                | Self::AssignmentShiftRight
                | Self::AssignmentUShiftRight
        )
    }

    pub fn is_constant_literal(self) -> bool {
        matches!(
            self,
            Self::Integer
                | Self::HexInteger
                | Self::BinaryInteger
                | Self::OctalInteger
                | Self::Float
                | Self::String
                | Self::RawString
                | Self::HashedString
                | Self::KeywordObjectSelf
                | Self::KeywordObjectInvalid
                | Self::KeywordJsonNull
                | Self::KeywordJsonFalse
                | Self::KeywordJsonTrue
                | Self::KeywordJsonObject
                | Self::KeywordJsonArray
                | Self::KeywordJsonString
                | Self::KeywordLocationInvalid
        )
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub token_type: TokenType,
    pub text: String,
    pub line: u32,
    pub col: u32,
    pub file_id: u32,
}

impl Token {
    pub fn new(token_type: TokenType, text: String, line: u32, col: u32, file_id: u32) -> Self {
        Self { token_type, text, line, col, file_id }
    }

    pub fn eof(line: u32, col: u32, file_id: u32) -> Self {
        Self { token_type: TokenType::Eof, text: String::new(), line, col, file_id }
    }
}
