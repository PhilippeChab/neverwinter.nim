use std::collections::HashMap;
use std::sync::Mutex;

use crate::errors::{CompileError, Diagnostic};
use crate::token::{Token, TokenType};

static ENGINE_STRUCTURE_KEYWORDS: Mutex<Option<HashMap<String, TokenType>>> = Mutex::new(None);

pub fn set_engine_structures(mappings: &[(u8, &str)]) {
    let mut map = HashMap::new();
    for &(idx, name) in mappings {
        let tt = match idx {
            0 => TokenType::KeywordEngineStructure0,
            1 => TokenType::KeywordEngineStructure1,
            2 => TokenType::KeywordEngineStructure2,
            3 => TokenType::KeywordEngineStructure3,
            4 => TokenType::KeywordEngineStructure4,
            5 => TokenType::KeywordEngineStructure5,
            6 => TokenType::KeywordEngineStructure6,
            7 => TokenType::KeywordEngineStructure7,
            8 => TokenType::KeywordEngineStructure8,
            9 => TokenType::KeywordEngineStructure9,
            _ => continue,
        };
        map.insert(name.to_string(), tt);
    }
    *ENGINE_STRUCTURE_KEYWORDS.lock().unwrap() = Some(map);
}

pub struct Lexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
    file_id: u32,
    pub diagnostics: Vec<Diagnostic>,
    pub file_name: String,
    pub brace_depth: i32,
    pub paren_depth: i32,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str, file_name: &str, file_id: u32) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            file_id,
            diagnostics: Vec::new(),
            file_name: file_name.to_string(),
            brace_depth: 0,
            paren_depth: 0,
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = tok.token_type == TokenType::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        tokens
    }

    fn peek(&self) -> u8 {
        if self.pos < self.source.len() {
            self.source[self.pos]
        } else {
            0
        }
    }

    fn peek_at(&self, offset: usize) -> u8 {
        let idx = self.pos + offset;
        if idx < self.source.len() {
            self.source[idx]
        } else {
            0
        }
    }

    fn advance(&mut self) -> u8 {
        if self.pos < self.source.len() {
            let ch = self.source[self.pos];
            self.pos += 1;
            if ch == b'\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            ch
        } else {
            0
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn error(&mut self, err: CompileError) {
        self.diagnostics.push(Diagnostic {
            severity: err.default_severity(),
            error: err,
            file: self.file_name.clone(),
            line: self.line,
            message: err.message().to_string(),
        });
    }

    fn skip_whitespace(&mut self) {
        while !self.at_end() {
            let ch = self.peek();
            if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while !self.at_end() && self.peek() != b'\n' {
            self.advance();
        }
    }

    fn skip_block_comment(&mut self) {
        loop {
            if self.at_end() {
                return;
            }
            if self.peek() == b'*' && self.peek_at(1) == b'/' {
                self.advance();
                self.advance();
                return;
            }
            self.advance();
        }
    }

    fn read_string(&mut self) -> Option<(String, TokenType)> {
        let mut buf = Vec::new();
        loop {
            if self.at_end() {
                self.error(CompileError::UnterminatedStringConstant);
                return None;
            }
            let ch = self.peek();
            if ch == b'\n' {
                self.error(CompileError::UnterminatedStringConstant);
                return None;
            }
            if ch == b'"' {
                self.advance();
                // C++ preserves every byte (including \xAB and other bytes >= 0x80)
                // verbatim in the token payload. `from_utf8_lossy` would replace
                // invalid sequences with U+FFFD and inflate `\xAB` to a 3-byte UTF-8
                // sequence — breaking codegen and hashed-string parity. The buffer
                // is only consumed via `.as_bytes()` downstream, so this is safe.
                let s = unsafe { String::from_utf8_unchecked(buf) };
                return Some((s, TokenType::String));
            }
            if ch == b'\\' {
                self.advance();
                if self.at_end() {
                    self.error(CompileError::UnterminatedStringConstant);
                    return None;
                }
                let esc = self.advance();
                match esc {
                    b'n' => buf.push(b'\n'),
                    b'\\' => buf.push(b'\\'),
                    b'"' => buf.push(b'"'),
                    b'x' => {
                        // C++ requires at least 2 chars after \x or unterminated string.
                        // The parse itself is strtol-lenient: bad hex digits accumulate
                        // a partial result (e.g. \x1G → 0x01, \xGZ → 0x00) and the byte
                        // is still emitted.
                        if self.pos + 2 > self.source.len() {
                            self.error(CompileError::UnterminatedStringConstant);
                            return None;
                        }
                        let h1 = self.advance();
                        let h2 = self.advance();
                        let to_hex = |c: u8| -> Option<u8> {
                            match c {
                                b'0'..=b'9' => Some(c - b'0'),
                                b'a'..=b'f' => Some(c - b'a' + 10),
                                b'A'..=b'F' => Some(c - b'A' + 10),
                                _ => None,
                            }
                        };
                        // C++ calls strtol on the 2-char window, which accepts a
                        // leading +/- sign (e.g. "\x-5" → strtol("-5",16) = -5 → 0xFB).
                        let val: u8 = if h1 == b'-' || h1 == b'+' {
                            let mag = to_hex(h2).unwrap_or(0) as i32;
                            let signed = if h1 == b'-' { -mag } else { mag };
                            signed as u8
                        } else {
                            // strtol stops at the first non-hex char: an invalid first
                            // nibble converts nothing (\xG1 -> 0), and a valid first with
                            // invalid second yields just the first nibble (\x1G -> 1).
                            match to_hex(h1) {
                                None => 0,
                                Some(d1) => match to_hex(h2) {
                                    Some(d2) => (d1 << 4) | d2,
                                    None => d1,
                                },
                            }
                        };
                        buf.push(val);
                    }
                    b'\n' => {
                        // C++: `\` followed immediately by newline → unterminated string
                        self.error(CompileError::UnterminatedStringConstant);
                        return None;
                    }
                    other => {
                        // C++: unknown escape — drop the backslash, keep only the next char
                        buf.push(other);
                    }
                }
            } else {
                buf.push(self.advance());
            }
        }
    }

    fn read_raw_string(&mut self) -> Option<(String, TokenType)> {
        let mut buf = Vec::new();
        loop {
            if self.at_end() {
                self.error(CompileError::UnterminatedStringConstant);
                return None;
            }
            let ch = self.peek();
            if ch == b'"' {
                self.advance();
                if self.peek() == b'"' {
                    self.advance();
                    buf.push(b'"');
                } else {
                    let s = unsafe { String::from_utf8_unchecked(buf) };
                    return Some((s, TokenType::String));
                }
            } else {
                buf.push(self.advance());
            }
        }
    }

    fn read_hashed_string(&mut self) -> Option<(String, TokenType)> {
        // C++ NWScript routes both TOKEN_STRING and TOKEN_HASHED_STRING through the
        // same ParseStringCharacter (scriptcomplexical.cpp:595-651). Reuse read_string
        // so escape handling stays consistent, then hash the result with CExoString::GetHash.
        let (s, _tt) = self.read_string()?;
        let hash = crate::xxh32::cexo_string_hash(&s);
        Some((format!("0x{:x}", hash as u32), TokenType::HexInteger))
    }

    fn read_number(&mut self, first: u8) -> Token {
        let start_line = self.line;
        let start_col = self.col - 1;
        let mut buf = vec![first];

        if first == b'0' {
            let next = self.peek();
            if next == b'x' || next == b'X' {
                buf.push(self.advance());
                while !self.at_end() && is_hex_digit(self.peek()) {
                    buf.push(self.advance());
                }
                // C++: after `0x`, any letter that is not a hex digit is
                // ERROR_UNEXPECTED_CHARACTER (scriptcomplexical.cpp:287-319).
                if !self.at_end() && self.peek().is_ascii_alphabetic() {
                    self.error(CompileError::UnexpectedCharacter);
                }
                return Token::new(
                    TokenType::HexInteger,
                    String::from_utf8_lossy(&buf).into_owned(),
                    start_line,
                    start_col,
                    self.file_id,
                );
            }
            if next == b'b' || next == b'B' {
                buf.push(self.advance());
                while !self.at_end() && (self.peek() == b'0' || self.peek() == b'1') {
                    buf.push(self.advance());
                }
                // C++ errors on any non-binary letter/digit immediately after a
                // binary integer prefix (scriptcomplexical.cpp:147-167).
                if !self.at_end() && (self.peek().is_ascii_alphanumeric()) {
                    self.error(CompileError::UnexpectedCharacter);
                }
                return Token::new(
                    TokenType::BinaryInteger,
                    String::from_utf8_lossy(&buf).into_owned(),
                    start_line,
                    start_col,
                    self.file_id,
                );
            }
            if next == b'o' || next == b'O' {
                buf.push(self.advance());
                while !self.at_end() && self.peek() >= b'0' && self.peek() <= b'7' {
                    buf.push(self.advance());
                }
                if !self.at_end() && (self.peek().is_ascii_alphanumeric()) {
                    self.error(CompileError::UnexpectedCharacter);
                }
                return Token::new(
                    TokenType::OctalInteger,
                    String::from_utf8_lossy(&buf).into_owned(),
                    start_line,
                    start_col,
                    self.file_id,
                );
            }
        }

        let mut is_float = false;
        while !self.at_end() && (self.peek().is_ascii_digit() || self.peek() == b'.') {
            if self.peek() == b'.' {
                if is_float {
                    // C++: second '.' in FLOAT state → UnexpectedCharacter
                    self.advance();
                    self.error(CompileError::UnexpectedCharacter);
                    break;
                }
                is_float = true;
            }
            buf.push(self.advance());
        }
        // C++ accepts only lowercase 'f' as the float suffix (scriptcomplexical.cpp:1691).
        if !self.at_end() && self.peek() == b'f' {
            is_float = true;
            self.advance();
        }

        Token::new(
            if is_float {
                TokenType::Float
            } else {
                TokenType::Integer
            },
            String::from_utf8_lossy(&buf).into_owned(),
            start_line,
            start_col,
            self.file_id,
        )
    }

    fn read_identifier(&mut self, first: u8) -> Token {
        let start_line = self.line;
        let start_col = self.col - 1;
        let mut buf = vec![first];

        while !self.at_end() && is_ident_char(self.peek()) {
            buf.push(self.advance());
        }

        let text = String::from_utf8_lossy(&buf).into_owned();
        let tt = keyword_lookup(&text).unwrap_or(TokenType::Identifier);

        Token::new(tt, text, start_line, start_col, self.file_id)
    }

    fn make_token(&self, tt: TokenType, text: &str, start_line: u32, start_col: u32) -> Token {
        Token::new(tt, text.to_string(), start_line, start_col, self.file_id)
    }

    pub fn next_token(&mut self) -> Token {
        loop {
            self.skip_whitespace();

            if self.at_end() {
                return Token::eof(self.line, self.col, self.file_id);
            }

            let start_line = self.line;
            let start_col = self.col;
            let ch = self.advance();

            match ch {
                b'/' => {
                    let next = self.peek();
                    if next == b'/' {
                        self.advance();
                        self.skip_line_comment();
                        continue;
                    }
                    if next == b'*' {
                        self.advance();
                        self.skip_block_comment();
                        continue;
                    }
                    if next == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentDivide, "/=", start_line, start_col);
                    }
                    return self.make_token(TokenType::Divide, "/", start_line, start_col);
                }

                b'{' => {
                    self.brace_depth += 1;
                    return self.make_token(TokenType::LeftBrace, "{", start_line, start_col);
                }
                b'}' => {
                    if self.brace_depth > 0 {
                        self.brace_depth -= 1;
                    }
                    return self.make_token(TokenType::RightBrace, "}", start_line, start_col);
                }
                b'(' => {
                    self.paren_depth += 1;
                    return self.make_token(TokenType::LeftBracket, "(", start_line, start_col);
                }
                b')' => {
                    if self.paren_depth > 0 {
                        self.paren_depth -= 1;
                    }
                    return self.make_token(TokenType::RightBracket, ")", start_line, start_col);
                }
                b'[' => return self.make_token(TokenType::LeftSquareBracket, "[", start_line, start_col),
                b']' => return self.make_token(TokenType::RightSquareBracket, "]", start_line, start_col),
                b';' => return self.make_token(TokenType::Semicolon, ";", start_line, start_col),
                b',' => return self.make_token(TokenType::Comma, ",", start_line, start_col),
                b'?' => return self.make_token(TokenType::QuestionMark, "?", start_line, start_col),
                b':' => return self.make_token(TokenType::Colon, ":", start_line, start_col),
                b'~' => return self.make_token(TokenType::Tilde, "~", start_line, start_col),

                b'+' => {
                    let next = self.peek();
                    if next == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentPlus, "+=", start_line, start_col);
                    }
                    if next == b'+' {
                        self.advance();
                        return self.make_token(TokenType::Increment, "++", start_line, start_col);
                    }
                    return self.make_token(TokenType::Plus, "+", start_line, start_col);
                }

                b'-' => {
                    let next = self.peek();
                    if next == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentMinus, "-=", start_line, start_col);
                    }
                    if next == b'-' {
                        self.advance();
                        return self.make_token(TokenType::Decrement, "--", start_line, start_col);
                    }
                    return self.make_token(TokenType::Minus, "-", start_line, start_col);
                }

                b'*' => {
                    if self.peek() == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentMultiply, "*=", start_line, start_col);
                    }
                    return self.make_token(TokenType::Multiply, "*", start_line, start_col);
                }

                b'%' => {
                    if self.peek() == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentModulus, "%=", start_line, start_col);
                    }
                    return self.make_token(TokenType::Modulus, "%", start_line, start_col);
                }

                b'&' => {
                    let next = self.peek();
                    if next == b'&' {
                        self.advance();
                        return self.make_token(TokenType::LogicalAnd, "&&", start_line, start_col);
                    }
                    if next == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentAnd, "&=", start_line, start_col);
                    }
                    return self.make_token(TokenType::BooleanAnd, "&", start_line, start_col);
                }

                b'|' => {
                    let next = self.peek();
                    if next == b'|' {
                        self.advance();
                        return self.make_token(TokenType::LogicalOr, "||", start_line, start_col);
                    }
                    if next == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentOr, "|=", start_line, start_col);
                    }
                    return self.make_token(TokenType::InclusiveOr, "|", start_line, start_col);
                }

                b'^' => {
                    if self.peek() == b'=' {
                        self.advance();
                        return self.make_token(TokenType::AssignmentXor, "^=", start_line, start_col);
                    }
                    return self.make_token(TokenType::ExclusiveOr, "^", start_line, start_col);
                }

                b'<' => {
                    let next = self.peek();
                    if next == b'=' {
                        self.advance();
                        return self.make_token(TokenType::CondLessEqual, "<=", start_line, start_col);
                    }
                    if next == b'<' {
                        self.advance();
                        if self.peek() == b'=' {
                            self.advance();
                            return self.make_token(TokenType::AssignmentShiftLeft, "<<=", start_line, start_col);
                        }
                        return self.make_token(TokenType::ShiftLeft, "<<", start_line, start_col);
                    }
                    return self.make_token(TokenType::CondLessThan, "<", start_line, start_col);
                }

                b'>' => {
                    let next = self.peek();
                    if next == b'=' {
                        self.advance();
                        return self.make_token(TokenType::CondGreaterEqual, ">=", start_line, start_col);
                    }
                    if next == b'>' {
                        self.advance();
                        let next2 = self.peek();
                        if next2 == b'>' {
                            self.advance();
                            if self.peek() == b'=' {
                                self.advance();
                                return self.make_token(TokenType::AssignmentUShiftRight, ">>>=", start_line, start_col);
                            }
                            return self.make_token(TokenType::UnsignedShiftRight, ">>>", start_line, start_col);
                        }
                        if next2 == b'=' {
                            self.advance();
                            return self.make_token(TokenType::AssignmentShiftRight, ">>=", start_line, start_col);
                        }
                        return self.make_token(TokenType::ShiftRight, ">>", start_line, start_col);
                    }
                    return self.make_token(TokenType::CondGreaterThan, ">", start_line, start_col);
                }

                b'!' => {
                    if self.peek() == b'=' {
                        self.advance();
                        return self.make_token(TokenType::CondNotEqual, "!=", start_line, start_col);
                    }
                    return self.make_token(TokenType::BooleanNot, "!", start_line, start_col);
                }

                b'=' => {
                    if self.peek() == b'=' {
                        self.advance();
                        return self.make_token(TokenType::CondEqual, "==", start_line, start_col);
                    }
                    return self.make_token(TokenType::AssignmentEqual, "=", start_line, start_col);
                }

                b'.' => {
                    if self.peek().is_ascii_digit() {
                        let tok = self.read_number(b'0');
                        return Token::new(
                            TokenType::Float,
                            format!("0.{}", &tok.text[1..]),
                            start_line,
                            start_col,
                            self.file_id,
                        );
                    }
                    return self.make_token(TokenType::StructurePartSpecify, ".", start_line, start_col);
                }

                b'"' => {
                    if let Some((text, tt)) = self.read_string() {
                        return Token::new(tt, text, start_line, start_col, self.file_id);
                    }
                    continue;
                }

                b'#' => {
                    let mut directive = Vec::new();
                    while !self.at_end() && is_ident_char(self.peek()) {
                        directive.push(self.advance());
                    }
                    let dir = String::from_utf8_lossy(&directive);
                    match dir.as_ref() {
                        "include" => {
                            return self.make_token(TokenType::KeywordInclude, "#include", start_line, start_col);
                        }
                        "define" => {
                            return self.make_token(TokenType::KeywordDefine, "#define", start_line, start_col);
                        }
                        _ => {
                            self.error(CompileError::UnexpectedCharacter);
                            continue;
                        }
                    }
                }

                ch if ch.is_ascii_digit() => {
                    return self.read_number(ch);
                }

                ch if is_ident_start(ch) => {
                    if (ch == b'r' || ch == b'R') && self.peek() == b'"' {
                        self.advance();
                        if let Some((text, tt)) = self.read_raw_string() {
                            return Token::new(tt, text, start_line, start_col, self.file_id);
                        }
                        continue;
                    }
                    if (ch == b'h' || ch == b'H') && self.peek() == b'"' {
                        self.advance();
                        if let Some((text, tt)) = self.read_hashed_string() {
                            return Token::new(tt, text, start_line, start_col, self.file_id);
                        }
                        continue;
                    }
                    return self.read_identifier(ch);
                }

                _ => {
                    // C++ scriptcomplexical.cpp:1607-1847 silently drops any byte it
                    // doesn't explicitly handle ($, @, `, \, 0x7F, and the UTF-8 BOM
                    // bytes 0xEF 0xBB 0xBF). Match that — common editors prepend a BOM
                    // and rejecting it would break LSP compatibility.
                    continue;
                }
            }
        }
    }
}

fn is_ident_start(ch: u8) -> bool {
    ch.is_ascii_alphabetic() || ch == b'_'
}

fn is_ident_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_'
}

fn is_hex_digit(ch: u8) -> bool {
    ch.is_ascii_hexdigit()
}

fn exo_hash(s: &str) -> u32 {
    let mut hash: u32 = 0;
    for &b in s.as_bytes() {
        let ch = b.to_ascii_lowercase() as u32;
        hash = hash.wrapping_mul(31).wrapping_add(ch);
    }
    hash
}

fn keyword_lookup(word: &str) -> Option<TokenType> {
    // Check dynamic engine structure keywords first
    if let Ok(guard) = ENGINE_STRUCTURE_KEYWORDS.lock() {
        if let Some(ref map) = *guard {
            if let Some(&tt) = map.get(word) {
                return Some(tt);
            }
        }
    }

    Some(match word {
        "int" => TokenType::KeywordInt,
        "float" => TokenType::KeywordFloat,
        "string" => TokenType::KeywordString,
        "object" => TokenType::KeywordObject,
        "void" => TokenType::KeywordVoid,
        "vector" => TokenType::KeywordVector,
        "struct" => TokenType::KeywordStruct,
        "action" => TokenType::KeywordAction,
        "const" => TokenType::KeywordConst,
        "if" => TokenType::KeywordIf,
        "else" => TokenType::KeywordElse,
        "while" => TokenType::KeywordWhile,
        "for" => TokenType::KeywordFor,
        "do" => TokenType::KeywordDo,
        "switch" => TokenType::KeywordSwitch,
        "case" => TokenType::KeywordCase,
        "default" => TokenType::KeywordDefault,
        "break" => TokenType::KeywordBreak,
        "continue" => TokenType::KeywordContinue,
        "return" => TokenType::KeywordReturn,
        "OBJECT_SELF" => TokenType::KeywordObjectSelf,
        "OBJECT_INVALID" => TokenType::KeywordObjectInvalid,
        // C++ NWScript only recognises the all-uppercase JSON_* literal keywords.
        "JSON_FALSE" => TokenType::KeywordJsonFalse,
        "JSON_TRUE" => TokenType::KeywordJsonTrue,
        "JSON_OBJECT" => TokenType::KeywordJsonObject,
        "JSON_ARRAY" => TokenType::KeywordJsonArray,
        "JSON_STRING" => TokenType::KeywordJsonString,
        "JSON_NULL" => TokenType::KeywordJsonNull,
        "LOCATION_INVALID" => TokenType::KeywordLocationInvalid,
        // Engine structure types are dynamically registered via
        // Lexer::set_engine_structures() from the lang spec's
        // #define ENGINE_STRUCTURE_N directives.
        "__FUNCTION__" => TokenType::KeywordDashDashFunction,
        "__FILE__" => TokenType::KeywordDashDashFile,
        "__LINE__" => TokenType::KeywordDashDashLine,
        "__DATE__" => TokenType::KeywordDashDashDate,
        "__TIME__" => TokenType::KeywordDashDashTime,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_source() {
        let mut lexer = Lexer::new("", "test.nss", 0);
        let tokens = lexer.tokenize();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_type, TokenType::Eof);
    }

    #[test]
    fn test_simple_function() {
        let src = "void main() { }";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let types: Vec<_> = tokens.iter().map(|t| t.token_type).collect();
        assert_eq!(
            types,
            vec![
                TokenType::KeywordVoid,
                TokenType::Identifier,
                TokenType::LeftBracket,
                TokenType::RightBracket,
                TokenType::LeftBrace,
                TokenType::RightBrace,
                TokenType::Eof,
            ]
        );
    }

    #[test]
    fn test_operators() {
        let src = "a + b - c * d / e % f";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type != TokenType::Identifier && t.token_type != TokenType::Eof)
            .map(|t| t.token_type)
            .collect();
        assert_eq!(
            ops,
            vec![
                TokenType::Plus,
                TokenType::Minus,
                TokenType::Multiply,
                TokenType::Divide,
                TokenType::Modulus,
            ]
        );
    }

    #[test]
    fn test_string_literal() {
        let src = r#""hello world""#;
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token_type, TokenType::String);
        assert_eq!(tokens[0].text, "hello world");
    }

    #[test]
    fn test_string_escape() {
        let src = r#""hello\nworld""#;
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].text, "hello\nworld");
    }

    #[test]
    fn test_hex_integer() {
        let src = "0xFF";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token_type, TokenType::HexInteger);
        assert_eq!(tokens[0].text, "0xFF");
    }

    #[test]
    fn test_binary_integer() {
        let src = "0b1010";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token_type, TokenType::BinaryInteger);
    }

    #[test]
    fn test_comments() {
        let src = "int x; // this is a comment\nint y; /* block comment */ int z;";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let idents: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TokenType::Identifier)
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(idents, vec!["x", "y", "z"]);
    }

    #[test]
    fn test_comparison_operators() {
        let src = "a >= b <= c > d < e != f == g";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| !matches!(t.token_type, TokenType::Identifier | TokenType::Eof))
            .map(|t| t.token_type)
            .collect();
        assert_eq!(
            ops,
            vec![
                TokenType::CondGreaterEqual,
                TokenType::CondLessEqual,
                TokenType::CondGreaterThan,
                TokenType::CondLessThan,
                TokenType::CondNotEqual,
                TokenType::CondEqual,
            ]
        );
    }

    #[test]
    fn test_shift_operators() {
        let src = "a << b >> c >>> d";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| !matches!(t.token_type, TokenType::Identifier | TokenType::Eof))
            .map(|t| t.token_type)
            .collect();
        assert_eq!(
            ops,
            vec![
                TokenType::ShiftLeft,
                TokenType::ShiftRight,
                TokenType::UnsignedShiftRight,
            ]
        );
    }

    #[test]
    fn test_assignment_operators() {
        let src = "= += -= *= /= %= &= ^= |= <<= >>= >>>=";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type != TokenType::Eof)
            .map(|t| t.token_type)
            .collect();
        assert_eq!(
            ops,
            vec![
                TokenType::AssignmentEqual,
                TokenType::AssignmentPlus,
                TokenType::AssignmentMinus,
                TokenType::AssignmentMultiply,
                TokenType::AssignmentDivide,
                TokenType::AssignmentModulus,
                TokenType::AssignmentAnd,
                TokenType::AssignmentXor,
                TokenType::AssignmentOr,
                TokenType::AssignmentShiftLeft,
                TokenType::AssignmentShiftRight,
                TokenType::AssignmentUShiftRight,
            ]
        );
    }

    #[test]
    fn test_brace_depth_tracking() {
        let src = "{ { } }";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let _ = lexer.tokenize();
        assert_eq!(lexer.brace_depth, 0);
    }

    #[test]
    fn test_include_directive() {
        let src = "#include \"somefile\"";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token_type, TokenType::KeywordInclude);
        assert_eq!(tokens[1].token_type, TokenType::String);
        assert_eq!(tokens[1].text, "somefile");
    }

    #[test]
    fn test_struct_field_access() {
        let src = "myStruct.field";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let types: Vec<_> = tokens.iter().map(|t| t.token_type).collect();
        assert_eq!(
            types,
            vec![
                TokenType::Identifier,
                TokenType::StructurePartSpecify,
                TokenType::Identifier,
                TokenType::Eof,
            ]
        );
    }

    #[test]
    fn test_float_starting_with_dot() {
        let src = ".42";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token_type, TokenType::Float);
    }

    #[test]
    fn test_keywords() {
        let src = "if else while for do switch case default break continue return const void int float string object vector struct action";
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let types: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type != TokenType::Eof)
            .map(|t| t.token_type)
            .collect();
        assert_eq!(
            types,
            vec![
                TokenType::KeywordIf,
                TokenType::KeywordElse,
                TokenType::KeywordWhile,
                TokenType::KeywordFor,
                TokenType::KeywordDo,
                TokenType::KeywordSwitch,
                TokenType::KeywordCase,
                TokenType::KeywordDefault,
                TokenType::KeywordBreak,
                TokenType::KeywordContinue,
                TokenType::KeywordReturn,
                TokenType::KeywordConst,
                TokenType::KeywordVoid,
                TokenType::KeywordInt,
                TokenType::KeywordFloat,
                TokenType::KeywordString,
                TokenType::KeywordObject,
                TokenType::KeywordVector,
                TokenType::KeywordStruct,
                TokenType::KeywordAction,
            ]
        );
    }
}
