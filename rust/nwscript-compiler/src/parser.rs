use crate::ast::{AstArena, AstNode, NodeId, NULL_NODE, Operation};
use crate::errors::{CompileError, Diagnostic};
use crate::token::{Token, TokenType};
use crate::types::NwType;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    pub arena: AstArena,
    pub diagnostics: Vec<Diagnostic>,
    pub file_names: Vec<String>,
    collect_all_errors: bool,
    require_entry_point: bool,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            arena: AstArena::new(),
            diagnostics: Vec::new(),
            file_names: Vec::new(),
            collect_all_errors: false,
            require_entry_point: true,
        }
    }

    pub fn set_collect_all_errors(&mut self, v: bool) {
        self.collect_all_errors = v;
    }

    pub fn set_require_entry_point(&mut self, v: bool) {
        self.require_entry_point = v;
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_type(&self) -> TokenType {
        self.peek().token_type
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, tt: TokenType) -> Result<&Token, CompileError> {
        if self.peek_type() == tt {
            Ok(self.advance())
        } else {
            Err(CompileError::UnexpectedCharacter)
        }
    }

    fn at_end(&self) -> bool {
        self.peek_type() == TokenType::Eof
    }

    fn error(&mut self, err: CompileError, tok: &Token) {
        let file = if (tok.file_id as usize) < self.file_names.len() {
            self.file_names[tok.file_id as usize].clone()
        } else {
            String::from("<unknown>")
        };
        self.diagnostics.push(Diagnostic {
            severity: err.default_severity(),
            error: err,
            file,
            line: tok.line,
            message: err.message().to_string(),
        });
    }

    fn make_node(&mut self, op: Operation) -> NodeId {
        let node = AstNode::new(op);
        self.arena.alloc(node)
    }

    fn make_node_at(&mut self, op: Operation, tok: &Token) -> NodeId {
        let mut node = AstNode::new(op);
        node.file_id = tok.file_id;
        node.line = tok.line;
        node.col = tok.col;
        self.arena.alloc(node)
    }

    pub fn parse_program(&mut self) -> Result<NodeId, CompileError> {
        let mut root = NULL_NODE;

        while !self.at_end() {
            match self.parse_functional_unit() {
                Ok(fu) => {
                    if root == NULL_NODE {
                        root = fu;
                    } else {
                        let last = self.find_rightmost(root);
                        self.arena.get_mut(last).right = fu;
                    }
                }
                Err(e) => {
                    if self.collect_all_errors {
                        let tok = self.peek().clone();
                        self.error(e, &tok);
                        self.synchronize();
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Ok(root)
    }

    fn find_rightmost(&self, mut node: NodeId) -> NodeId {
        loop {
            let right = self.arena.get(node).right;
            if right == NULL_NODE {
                return node;
            }
            node = right;
        }
    }

    fn synchronize(&mut self) {
        while !self.at_end() {
            match self.peek_type() {
                TokenType::RightBrace => {
                    self.advance();
                    if self.brace_depth() == 0 {
                        return;
                    }
                }
                TokenType::Semicolon => {
                    self.advance();
                    if self.brace_depth() == 0 {
                        return;
                    }
                }
                tt if tt.is_type_specifier() && self.brace_depth() == 0 => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn brace_depth(&self) -> i32 {
        let mut depth = 0i32;
        for i in 0..self.pos {
            match self.tokens[i].token_type {
                TokenType::LeftBrace => depth += 1,
                TokenType::RightBrace => depth -= 1,
                _ => {}
            }
        }
        depth.max(0)
    }

    fn parse_functional_unit(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();

        if tok.token_type == TokenType::KeywordInclude {
            return self.parse_include();
        }

        if tok.token_type == TokenType::KeywordStruct {
            if self.is_struct_definition() {
                return self.parse_struct_definition();
            }
        }

        if tok.token_type == TokenType::KeywordConst {
            return self.parse_const_declaration();
        }

        if tok.token_type.is_type_specifier() || tok.token_type == TokenType::Identifier {
            return self.parse_function_or_global_var();
        }

        Err(CompileError::InvalidDeclarationType)
    }

    fn parse_include(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordInclude)?;
        let name_tok = self.advance().clone();
        if name_tok.token_type != TokenType::String {
            return Err(CompileError::FileNotFound);
        }

        let node = self.make_node_at(Operation::FunctionalUnit, &tok);
        let mut n = AstNode::new(Operation::FunctionalUnit);
        n.string_data = Some(name_tok.text.clone());
        n.file_id = tok.file_id;
        n.line = tok.line;
        n.col = tok.col;
        n.int_data[0] = 1; // marks as include
        let include_id = self.arena.alloc(n);
        self.arena.get_mut(node).left = include_id;
        Ok(node)
    }

    fn is_struct_definition(&self) -> bool {
        let mut i = self.pos + 1;
        if i < self.tokens.len() && self.tokens[i].token_type == TokenType::Identifier {
            i += 1;
            if i < self.tokens.len() && self.tokens[i].token_type == TokenType::LeftBrace {
                return true;
            }
        }
        false
    }

    fn parse_struct_definition(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordStruct)?;

        let name_tok = self.advance().clone();
        if name_tok.token_type != TokenType::Identifier {
            return Err(CompileError::FunctionDefinitionMissingName);
        }

        self.expect(TokenType::LeftBrace)?;

        let fu = self.make_node_at(Operation::FunctionalUnit, &tok);
        let struct_node = self.make_node_at(Operation::KeywordStruct, &tok);
        let struct_def = self.make_node_at(Operation::StructureDefinition, &name_tok);
        self.arena.get_mut(struct_def).string_data = Some(name_tok.text.clone());

        let mut field_chain = NULL_NODE;

        while self.peek_type() != TokenType::RightBrace && !self.at_end() {
            let field_type = self.parse_type_specifier()?;
            let field_name_tok = self.advance().clone();
            if field_name_tok.token_type != TokenType::Identifier {
                return Err(CompileError::BadVariableName);
            }
            self.expect(TokenType::Semicolon)?;

            let var_node = self.make_node_at(Operation::Variable, &field_name_tok);
            self.arena.get_mut(var_node).string_data = Some(field_name_tok.text.clone());
            self.arena.get_mut(var_node).nw_type = field_type.0;
            if let Some(tn) = &field_type.1 {
                self.arena.get_mut(var_node).type_name = Some(tn.clone());
            }

            let vl = self.make_node(Operation::VariableList);
            self.arena.get_mut(vl).left = var_node;

            if field_chain == NULL_NODE {
                field_chain = vl;
            } else {
                let last = self.find_rightmost(field_chain);
                self.arena.get_mut(last).right = vl;
            }
        }

        self.expect(TokenType::RightBrace)?;
        self.expect(TokenType::Semicolon)?;

        self.arena.get_mut(struct_def).left = field_chain;
        self.arena.get_mut(struct_node).left = struct_def;
        self.arena.get_mut(fu).left = struct_node;
        Ok(fu)
    }

    fn parse_type_specifier(&mut self) -> Result<(NwType, Option<String>), CompileError> {
        let tok = self.advance().clone();
        match tok.token_type {
            TokenType::KeywordInt => Ok((NwType::Integer, None)),
            TokenType::KeywordFloat => Ok((NwType::Float, None)),
            TokenType::KeywordString => Ok((NwType::String, None)),
            TokenType::KeywordObject => Ok((NwType::Object, None)),
            TokenType::KeywordVoid => Ok((NwType::Void, None)),
            TokenType::KeywordVector => Ok((NwType::Vector, None)),
            TokenType::KeywordAction => Ok((NwType::Action, None)),
            TokenType::KeywordStruct => {
                let name = self.advance().clone();
                Ok((NwType::Struct, Some(name.text.clone())))
            }
            tt if matches!(
                tt,
                TokenType::KeywordEngineStructure0
                    | TokenType::KeywordEngineStructure1
                    | TokenType::KeywordEngineStructure2
                    | TokenType::KeywordEngineStructure3
                    | TokenType::KeywordEngineStructure4
                    | TokenType::KeywordEngineStructure5
                    | TokenType::KeywordEngineStructure6
                    | TokenType::KeywordEngineStructure7
                    | TokenType::KeywordEngineStructure8
                    | TokenType::KeywordEngineStructure9
            ) =>
            {
                let n = tt as u8 - TokenType::KeywordEngineStructure0 as u8;
                Ok((NwType::EngineStructure(n), None))
            }
            TokenType::Identifier => {
                Ok((NwType::Struct, Some(tok.text.clone())))
            }
            _ => Err(CompileError::BadTypeSpecifier),
        }
    }

    fn parse_const_declaration(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordConst)?;
        let type_info = self.parse_type_specifier()?;
        let name_tok = self.advance().clone();
        if name_tok.token_type != TokenType::Identifier {
            return Err(CompileError::BadVariableName);
        }

        self.expect(TokenType::AssignmentEqual)?;
        let value = self.parse_expression()?;
        self.expect(TokenType::Semicolon)?;

        let fu = self.make_node_at(Operation::FunctionalUnit, &tok);
        let const_decl = self.make_node_at(Operation::ConstDeclaration, &tok);
        self.arena.get_mut(const_decl).string_data = Some(name_tok.text.clone());
        self.arena.get_mut(const_decl).nw_type = type_info.0;
        if let Some(tn) = type_info.1 {
            self.arena.get_mut(const_decl).type_name = Some(tn);
        }
        self.arena.get_mut(const_decl).left = value;
        self.arena.get_mut(fu).left = const_decl;

        Ok(fu)
    }

    fn parse_function_or_global_var(&mut self) -> Result<NodeId, CompileError> {
        let type_info = self.parse_type_specifier()?;
        let name_tok = self.advance().clone();

        if name_tok.token_type != TokenType::Identifier {
            return Err(CompileError::FunctionDefinitionMissingName);
        }

        if self.peek_type() == TokenType::LeftBracket {
            return self.parse_function_def(type_info, &name_tok);
        }

        self.parse_global_variable(type_info, &name_tok)
    }

    fn parse_function_def(
        &mut self,
        return_type: (NwType, Option<String>),
        name_tok: &Token,
    ) -> Result<NodeId, CompileError> {
        self.expect(TokenType::LeftBracket)?;

        let fu = self.make_node_at(Operation::FunctionalUnit, name_tok);

        let func_id = self.make_node_at(Operation::FunctionIdentifier, name_tok);
        self.arena.get_mut(func_id).string_data = Some(name_tok.text.clone());
        self.arena.get_mut(func_id).nw_type = return_type.0;
        if let Some(tn) = &return_type.1 {
            self.arena.get_mut(func_id).type_name = Some(tn.clone());
        }

        let params = self.parse_parameter_list()?;
        self.expect(TokenType::RightBracket)?;

        if self.peek_type() == TokenType::Semicolon {
            self.advance();
            let decl = self.make_node_at(Operation::FunctionDeclaration, name_tok);
            self.arena.get_mut(decl).left = func_id;
            self.arena.get_mut(func_id).left = params;
            self.arena.get_mut(fu).left = decl;
            return Ok(fu);
        }

        if self.peek_type() == TokenType::LeftBrace {
            let body = self.parse_compound_statement()?;
            let func = self.make_node_at(Operation::Function, name_tok);
            self.arena.get_mut(func).left = func_id;
            self.arena.get_mut(func_id).left = params;
            self.arena.get_mut(func).right = body;
            self.arena.get_mut(fu).left = func;
            return Ok(fu);
        }

        Err(CompileError::FunctionDefinitionMissingParameterList)
    }

    fn parse_parameter_list(&mut self) -> Result<NodeId, CompileError> {
        if self.peek_type() == TokenType::RightBracket {
            return Ok(NULL_NODE);
        }

        let mut first = NULL_NODE;
        let mut had_optional = false;

        loop {
            let type_info = self.parse_type_specifier()?;
            let name_tok = self.advance().clone();
            if name_tok.token_type != TokenType::Identifier {
                return Err(CompileError::MalformedParameterList);
            }

            let param = self.make_node_at(Operation::FunctionParamName, &name_tok);
            self.arena.get_mut(param).string_data = Some(name_tok.text.clone());
            self.arena.get_mut(param).nw_type = type_info.0;
            if let Some(tn) = type_info.1 {
                self.arena.get_mut(param).type_name = Some(tn);
            }

            if self.peek_type() == TokenType::AssignmentEqual {
                self.advance();
                had_optional = true;
                let default_val = self.parse_expression()?;
                self.arena.get_mut(param).left = default_val;
            } else if had_optional {
                let tok = self.peek().clone();
                self.error(CompileError::NonOptionalParameterCannotFollowOptionalParameter, &tok);
            }

            if first == NULL_NODE {
                first = param;
            } else {
                let last = self.find_rightmost(first);
                self.arena.get_mut(last).right = param;
            }

            if self.peek_type() != TokenType::Comma {
                break;
            }
            self.advance();
        }

        Ok(first)
    }

    fn parse_global_variable(
        &mut self,
        type_info: (NwType, Option<String>),
        name_tok: &Token,
    ) -> Result<NodeId, CompileError> {
        let fu = self.make_node_at(Operation::FunctionalUnit, name_tok);
        let gv = self.make_node_at(Operation::GlobalVariables, name_tok);
        let decl = self.make_node_at(Operation::KeywordDeclaration, name_tok);

        let type_node = self.make_type_node(type_info.0, name_tok);
        if let Some(tn) = &type_info.1 {
            self.arena.get_mut(type_node).type_name = Some(tn.clone());
        }

        let var = self.make_node_at(Operation::Variable, name_tok);
        self.arena.get_mut(var).string_data = Some(name_tok.text.clone());

        if self.peek_type() == TokenType::AssignmentEqual {
            self.advance();
            let init = self.parse_expression()?;
            self.arena.get_mut(var).left = init;
        }

        self.expect(TokenType::Semicolon)?;

        let vl = self.make_node(Operation::VariableList);
        self.arena.get_mut(vl).left = var;
        self.arena.get_mut(decl).left = type_node;
        self.arena.get_mut(type_node).left = vl;
        self.arena.get_mut(gv).left = decl;
        self.arena.get_mut(fu).left = gv;

        Ok(fu)
    }

    fn make_type_node(&mut self, nw_type: NwType, tok: &Token) -> NodeId {
        let op = match nw_type {
            NwType::Integer => Operation::KeywordInt,
            NwType::Float => Operation::KeywordFloat,
            NwType::String => Operation::KeywordString,
            NwType::Object => Operation::KeywordObject,
            NwType::Void => Operation::KeywordVoid,
            NwType::Vector => Operation::KeywordVector,
            NwType::Struct => Operation::KeywordStruct,
            NwType::Action => Operation::KeywordVoid,
            NwType::EngineStructure(n) => Operation::KeywordEngineStructure(n),
        };
        self.make_node_at(op, tok)
    }

    fn parse_compound_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::LeftBrace)?;

        let cs = self.make_node_at(Operation::CompoundStatement, &tok);
        let mut stmts = NULL_NODE;

        while self.peek_type() != TokenType::RightBrace && !self.at_end() {
            match self.parse_statement() {
                Ok(stmt) => {
                    let sl = self.make_node(Operation::StatementList);
                    self.arena.get_mut(sl).left = stmt;
                    if stmts == NULL_NODE {
                        stmts = sl;
                    } else {
                        let last = self.find_rightmost(stmts);
                        self.arena.get_mut(last).right = sl;
                    }
                }
                Err(e) => {
                    if self.collect_all_errors {
                        let tok = self.peek().clone();
                        self.error(e, &tok);
                        self.sync_to_statement();
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        self.expect(TokenType::RightBrace)?;
        self.arena.get_mut(cs).left = stmts;
        Ok(cs)
    }

    fn sync_to_statement(&mut self) {
        let mut depth = 0i32;
        while !self.at_end() {
            match self.peek_type() {
                TokenType::Semicolon => {
                    self.advance();
                    if depth <= 0 {
                        return;
                    }
                }
                TokenType::LeftBrace => {
                    depth += 1;
                    self.advance();
                }
                TokenType::RightBrace => {
                    if depth <= 0 {
                        return;
                    }
                    depth -= 1;
                    self.advance();
                    if depth <= 0 {
                        return;
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn parse_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        let stmt_node = self.make_node_at(Operation::Statement, &tok);

        let inner = match tok.token_type {
            TokenType::LeftBrace => self.parse_compound_statement()?,
            TokenType::KeywordIf => self.parse_if_statement()?,
            TokenType::KeywordWhile => self.parse_while_statement()?,
            TokenType::KeywordDo => self.parse_do_while_statement()?,
            TokenType::KeywordFor => self.parse_for_statement()?,
            TokenType::KeywordSwitch => self.parse_switch_statement()?,
            TokenType::KeywordReturn => self.parse_return_statement()?,
            TokenType::KeywordBreak => {
                self.advance();
                self.expect(TokenType::Semicolon)?;
                self.make_node_at(Operation::Break, &tok)
            }
            TokenType::KeywordContinue => {
                self.advance();
                self.expect(TokenType::Semicolon)?;
                self.make_node_at(Operation::Continue, &tok)
            }
            tt if tt.is_non_void_type_specifier()
                || tt == TokenType::KeywordConst
                || tt == TokenType::KeywordStruct =>
            {
                self.parse_local_declaration()?
            }
            _ => {
                let expr = self.parse_expression()?;
                self.expect(TokenType::Semicolon)?;
                expr
            }
        };

        self.arena.get_mut(stmt_node).left = inner;
        Ok(stmt_node)
    }

    fn parse_if_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordIf)?;
        self.expect(TokenType::LeftBracket)?;

        let cond_expr = self.parse_expression()?;
        self.expect(TokenType::RightBracket)?;

        let body = self.parse_statement()?;

        let if_block = self.make_node_at(Operation::IfBlock, &tok);
        let if_cond = self.make_node_at(Operation::IfCondition, &tok);
        let if_choice = self.make_node_at(Operation::IfChoice, &tok);

        self.arena.get_mut(if_cond).left = cond_expr;
        self.arena.get_mut(if_choice).left = body;

        if self.peek_type() == TokenType::KeywordElse {
            self.advance();
            let else_body = self.parse_statement()?;
            self.arena.get_mut(if_choice).right = else_body;
        }

        self.arena.get_mut(if_block).left = if_cond;
        self.arena.get_mut(if_block).right = if_choice;
        Ok(if_block)
    }

    fn parse_while_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordWhile)?;
        self.expect(TokenType::LeftBracket)?;

        let cond = self.parse_expression()?;
        self.expect(TokenType::RightBracket)?;

        let body = self.parse_statement()?;

        let while_block = self.make_node_at(Operation::WhileBlock, &tok);
        let while_cond = self.make_node_at(Operation::WhileCondition, &tok);
        let while_choice = self.make_node_at(Operation::WhileChoice, &tok);

        self.arena.get_mut(while_cond).left = cond;
        self.arena.get_mut(while_choice).left = body;
        self.arena.get_mut(while_block).left = while_cond;
        self.arena.get_mut(while_block).right = while_choice;

        Ok(while_block)
    }

    fn parse_do_while_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordDo)?;

        let body = self.parse_statement()?;

        self.expect(TokenType::KeywordWhile)
            .map_err(|_| CompileError::NoWhileAfterDoKeyword)?;
        self.expect(TokenType::LeftBracket)?;
        let cond = self.parse_expression()?;
        self.expect(TokenType::RightBracket)?;
        self.expect(TokenType::Semicolon)?;

        let dw_block = self.make_node_at(Operation::DoWhileBlock, &tok);
        let dw_cond = self.make_node_at(Operation::DoWhileCondition, &tok);

        self.arena.get_mut(dw_cond).left = cond;
        self.arena.get_mut(dw_block).left = body;
        self.arena.get_mut(dw_block).right = dw_cond;

        Ok(dw_block)
    }

    fn parse_for_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordFor)?;
        self.expect(TokenType::LeftBracket)?;

        let init = if self.peek_type() == TokenType::Semicolon {
            self.advance();
            NULL_NODE
        } else if self.peek_type().is_non_void_type_specifier() || self.peek_type() == TokenType::KeywordStruct {
            self.parse_local_declaration()?
        } else {
            let e = self.parse_expression()?;
            self.expect(TokenType::Semicolon)?;
            e
        };

        let cond = if self.peek_type() == TokenType::Semicolon {
            NULL_NODE
        } else {
            self.parse_expression()?
        };
        self.expect(TokenType::Semicolon)?;

        let update = if self.peek_type() == TokenType::RightBracket {
            NULL_NODE
        } else {
            self.parse_expression()?
        };
        self.expect(TokenType::RightBracket)?;

        let body = self.parse_statement()?;

        let for_block = self.make_node_at(Operation::ForBlock, &tok);
        self.arena.get_mut(for_block).int_data[0] = if init != NULL_NODE { 1 } else { 0 };

        let cs = self.make_node_at(Operation::CompoundStatement, &tok);
        let while_block = self.make_node_at(Operation::WhileBlock, &tok);
        let while_cond = self.make_node_at(Operation::WhileCondition, &tok);
        let while_choice = self.make_node_at(Operation::WhileChoice, &tok);
        let while_cont = self.make_node_at(Operation::WhileContinue, &tok);

        self.arena.get_mut(while_cond).left = cond;
        self.arena.get_mut(while_cont).left = update;

        let body_sl = self.make_node(Operation::StatementList);
        self.arena.get_mut(body_sl).left = body;
        let cont_sl = self.make_node(Operation::StatementList);
        self.arena.get_mut(cont_sl).left = while_cont;
        self.arena.get_mut(body_sl).right = cont_sl;

        let inner_cs = self.make_node(Operation::CompoundStatement);
        self.arena.get_mut(inner_cs).left = body_sl;
        self.arena.get_mut(while_choice).left = inner_cs;

        self.arena.get_mut(while_block).left = while_cond;
        self.arena.get_mut(while_block).right = while_choice;

        if init != NULL_NODE {
            let init_sl = self.make_node(Operation::StatementList);
            let init_stmt = self.make_node(Operation::Statement);
            self.arena.get_mut(init_stmt).left = init;
            self.arena.get_mut(init_sl).left = init_stmt;
            let loop_sl = self.make_node(Operation::StatementList);
            let loop_stmt = self.make_node(Operation::StatementNoDebug);
            self.arena.get_mut(loop_stmt).left = while_block;
            self.arena.get_mut(loop_sl).left = loop_stmt;
            self.arena.get_mut(init_sl).right = loop_sl;
            self.arena.get_mut(cs).left = init_sl;
        } else {
            let sl = self.make_node(Operation::StatementList);
            self.arena.get_mut(sl).left = while_block;
            self.arena.get_mut(cs).left = sl;
        }

        self.arena.get_mut(for_block).left = cs;
        Ok(for_block)
    }

    fn parse_switch_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordSwitch)?;
        self.expect(TokenType::LeftBracket)?;

        let cond = self.parse_expression()?;
        self.expect(TokenType::RightBracket)?;
        self.expect(TokenType::LeftBrace)?;

        let switch_block = self.make_node_at(Operation::SwitchBlock, &tok);
        let switch_cond = self.make_node_at(Operation::SwitchCondition, &tok);
        self.arena.get_mut(switch_cond).left = cond;
        self.arena.get_mut(switch_block).left = switch_cond;

        let mut stmts = NULL_NODE;

        while self.peek_type() != TokenType::RightBrace && !self.at_end() {
            let case_tok = self.peek().clone();
            let case_node = if case_tok.token_type == TokenType::KeywordCase {
                self.advance();
                let val = self.parse_expression()?;
                self.expect(TokenType::Colon)?;
                let cn = self.make_node_at(Operation::Case, &case_tok);
                self.arena.get_mut(cn).left = val;
                cn
            } else if case_tok.token_type == TokenType::KeywordDefault {
                self.advance();
                self.expect(TokenType::Colon)?;
                self.make_node_at(Operation::Default, &case_tok)
            } else {
                let stmt = self.parse_statement()?;
                let sl = self.make_node(Operation::StatementList);
                self.arena.get_mut(sl).left = stmt;
                if stmts == NULL_NODE {
                    stmts = sl;
                } else {
                    let last = self.find_rightmost(stmts);
                    self.arena.get_mut(last).right = sl;
                }
                continue;
            };

            let sl = self.make_node(Operation::StatementList);
            self.arena.get_mut(sl).left = case_node;
            if stmts == NULL_NODE {
                stmts = sl;
            } else {
                let last = self.find_rightmost(stmts);
                self.arena.get_mut(last).right = sl;
            }
        }

        self.expect(TokenType::RightBrace)?;
        self.arena.get_mut(switch_block).right = stmts;
        Ok(switch_block)
    }

    fn parse_return_statement(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();
        self.expect(TokenType::KeywordReturn)?;

        let ret = self.make_node_at(Operation::Return, &tok);

        if self.peek_type() != TokenType::Semicolon {
            let expr = self.parse_expression()?;
            self.arena.get_mut(ret).left = expr;
        }

        self.expect(TokenType::Semicolon)?;
        Ok(ret)
    }

    fn parse_local_declaration(&mut self) -> Result<NodeId, CompileError> {
        let is_const = self.peek_type() == TokenType::KeywordConst;
        if is_const {
            self.advance();
        }

        let type_info = self.parse_type_specifier()?;
        let first_tok = self.peek().clone();

        let decl = self.make_node_at(
            if is_const { Operation::ConstDeclaration } else { Operation::KeywordDeclaration },
            &first_tok,
        );

        let type_node = self.make_type_node(type_info.0, &first_tok);
        if let Some(tn) = &type_info.1 {
            self.arena.get_mut(type_node).type_name = Some(tn.clone());
        }

        let mut var_chain = NULL_NODE;

        loop {
            let name_tok = self.advance().clone();
            if name_tok.token_type != TokenType::Identifier {
                return Err(CompileError::BadVariableName);
            }

            let var = self.make_node_at(Operation::Variable, &name_tok);
            self.arena.get_mut(var).string_data = Some(name_tok.text.clone());

            if self.peek_type() == TokenType::AssignmentEqual {
                self.advance();
                let init = self.parse_expression()?;
                self.arena.get_mut(var).left = init;
            }

            let vl = self.make_node(Operation::VariableList);
            self.arena.get_mut(vl).left = var;

            if var_chain == NULL_NODE {
                var_chain = vl;
            } else {
                let last = self.find_rightmost(var_chain);
                self.arena.get_mut(last).right = vl;
            }

            if self.peek_type() != TokenType::Comma {
                break;
            }
            self.advance(); // consume comma
        }

        self.expect(TokenType::Semicolon)?;

        self.arena.get_mut(decl).left = type_node;
        self.arena.get_mut(type_node).left = var_chain;

        Ok(decl)
    }

    // Expression parsing using precedence climbing
    pub fn parse_expression(&mut self) -> Result<NodeId, CompileError> {
        self.parse_assignment_expr()
    }

    fn parse_assignment_expr(&mut self) -> Result<NodeId, CompileError> {
        let left = self.parse_ternary_expr()?;

        if self.peek_type().is_assignment_operator() {
            let op_tok = self.advance().clone();
            let right = self.parse_assignment_expr()?;
            let assign = self.make_node_at(Operation::Assignment, &op_tok);
            self.arena.get_mut(assign).int_data[0] = op_tok.token_type as i32;
            self.arena.get_mut(assign).left = left;
            self.arena.get_mut(assign).right = right;
            return Ok(assign);
        }

        Ok(left)
    }

    fn parse_ternary_expr(&mut self) -> Result<NodeId, CompileError> {
        let cond = self.parse_logical_or_expr()?;

        if self.peek_type() == TokenType::QuestionMark {
            let tok = self.advance().clone();
            let then_expr = self.parse_expression()?;
            self.expect(TokenType::Colon)?;
            let else_expr = self.parse_ternary_expr()?;

            let cond_block = self.make_node_at(Operation::CondBlock, &tok);
            let cond_cond = self.make_node_at(Operation::CondCondition, &tok);
            let cond_choice = self.make_node_at(Operation::CondChoice, &tok);

            self.arena.get_mut(cond_cond).left = cond;
            self.arena.get_mut(cond_choice).left = then_expr;
            self.arena.get_mut(cond_choice).right = else_expr;
            self.arena.get_mut(cond_block).left = cond_cond;
            self.arena.get_mut(cond_block).right = cond_choice;
            return Ok(cond_block);
        }

        Ok(cond)
    }

    fn parse_logical_or_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_logical_and_expr()?;
        while self.peek_type() == TokenType::LogicalOr {
            let tok = self.advance().clone();
            let right = self.parse_logical_and_expr()?;
            let node = self.make_node_at(Operation::LogicalOr, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_logical_and_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_inclusive_or_expr()?;
        while self.peek_type() == TokenType::LogicalAnd {
            let tok = self.advance().clone();
            let right = self.parse_inclusive_or_expr()?;
            let node = self.make_node_at(Operation::LogicalAnd, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_inclusive_or_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_exclusive_or_expr()?;
        while self.peek_type() == TokenType::InclusiveOr {
            let tok = self.advance().clone();
            let right = self.parse_exclusive_or_expr()?;
            let node = self.make_node_at(Operation::InclusiveOr, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_exclusive_or_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_boolean_and_expr()?;
        while self.peek_type() == TokenType::ExclusiveOr {
            let tok = self.advance().clone();
            let right = self.parse_boolean_and_expr()?;
            let node = self.make_node_at(Operation::ExclusiveOr, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_boolean_and_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_equality_expr()?;
        while self.peek_type() == TokenType::BooleanAnd {
            let tok = self.advance().clone();
            let right = self.parse_equality_expr()?;
            let node = self.make_node_at(Operation::BooleanAnd, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_equality_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_relational_expr()?;
        while matches!(self.peek_type(), TokenType::CondEqual | TokenType::CondNotEqual) {
            let tok = self.advance().clone();
            let right = self.parse_relational_expr()?;
            let op = if tok.token_type == TokenType::CondEqual {
                Operation::ConditionEqual
            } else {
                Operation::ConditionNotEqual
            };
            let node = self.make_node_at(op, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_relational_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_shift_expr()?;
        while matches!(
            self.peek_type(),
            TokenType::CondLessThan
                | TokenType::CondGreaterThan
                | TokenType::CondLessEqual
                | TokenType::CondGreaterEqual
        ) {
            let tok = self.advance().clone();
            let right = self.parse_shift_expr()?;
            let op = match tok.token_type {
                TokenType::CondLessThan => Operation::ConditionLT,
                TokenType::CondGreaterThan => Operation::ConditionGT,
                TokenType::CondLessEqual => Operation::ConditionLEQ,
                TokenType::CondGreaterEqual => Operation::ConditionGEQ,
                _ => unreachable!(),
            };
            let node = self.make_node_at(op, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_shift_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_additive_expr()?;
        while matches!(
            self.peek_type(),
            TokenType::ShiftLeft | TokenType::ShiftRight | TokenType::UnsignedShiftRight
        ) {
            let tok = self.advance().clone();
            let right = self.parse_additive_expr()?;
            let op = match tok.token_type {
                TokenType::ShiftLeft => Operation::ShiftLeft,
                TokenType::ShiftRight => Operation::ShiftRight,
                TokenType::UnsignedShiftRight => Operation::UnsignedShiftRight,
                _ => unreachable!(),
            };
            let node = self.make_node_at(op, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_additive_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_multiplicative_expr()?;
        while matches!(self.peek_type(), TokenType::Plus | TokenType::Minus) {
            let tok = self.advance().clone();
            let right = self.parse_multiplicative_expr()?;
            let op = if tok.token_type == TokenType::Plus {
                Operation::Add
            } else {
                Operation::Subtract
            };
            let node = self.make_node_at(op, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_multiplicative_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_unary_expr()?;
        while matches!(
            self.peek_type(),
            TokenType::Multiply | TokenType::Divide | TokenType::Modulus
        ) {
            let tok = self.advance().clone();
            let right = self.parse_unary_expr()?;
            let op = match tok.token_type {
                TokenType::Multiply => Operation::Multiply,
                TokenType::Divide => Operation::Divide,
                TokenType::Modulus => Operation::Modulus,
                _ => unreachable!(),
            };
            let node = self.make_node_at(op, &tok);
            self.arena.get_mut(node).left = left;
            self.arena.get_mut(node).right = right;
            left = node;
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self) -> Result<NodeId, CompileError> {
        match self.peek_type() {
            TokenType::Minus => {
                let tok = self.advance().clone();
                let operand = self.parse_unary_expr()?;
                let node = self.make_node_at(Operation::Negation, &tok);
                self.arena.get_mut(node).left = operand;
                Ok(node)
            }
            TokenType::BooleanNot => {
                let tok = self.advance().clone();
                let operand = self.parse_unary_expr()?;
                let node = self.make_node_at(Operation::BooleanNot, &tok);
                self.arena.get_mut(node).left = operand;
                Ok(node)
            }
            TokenType::Tilde => {
                let tok = self.advance().clone();
                let operand = self.parse_unary_expr()?;
                let node = self.make_node_at(Operation::OnesComplement, &tok);
                self.arena.get_mut(node).left = operand;
                Ok(node)
            }
            TokenType::Increment => {
                let tok = self.advance().clone();
                let operand = self.parse_unary_expr()?;
                let node = self.make_node_at(Operation::PreIncrement, &tok);
                self.arena.get_mut(node).left = operand;
                Ok(node)
            }
            TokenType::Decrement => {
                let tok = self.advance().clone();
                let operand = self.parse_unary_expr()?;
                let node = self.make_node_at(Operation::PreDecrement, &tok);
                self.arena.get_mut(node).left = operand;
                Ok(node)
            }
            _ => self.parse_postfix_expr(),
        }
    }

    fn parse_postfix_expr(&mut self) -> Result<NodeId, CompileError> {
        let mut left = self.parse_primary_expr()?;

        loop {
            match self.peek_type() {
                TokenType::Increment => {
                    let tok = self.advance().clone();
                    let node = self.make_node_at(Operation::PostIncrement, &tok);
                    self.arena.get_mut(node).left = left;
                    left = node;
                }
                TokenType::Decrement => {
                    let tok = self.advance().clone();
                    let node = self.make_node_at(Operation::PostDecrement, &tok);
                    self.arena.get_mut(node).left = left;
                    left = node;
                }
                TokenType::StructurePartSpecify => {
                    let tok = self.advance().clone();
                    let field_tok = self.advance().clone();
                    let node = self.make_node_at(Operation::StructurePart, &tok);
                    self.arena.get_mut(node).left = left;
                    self.arena.get_mut(node).string_data = Some(field_tok.text.clone());
                    left = node;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    fn parse_primary_expr(&mut self) -> Result<NodeId, CompileError> {
        let tok = self.peek().clone();

        match tok.token_type {
            TokenType::Integer | TokenType::HexInteger | TokenType::BinaryInteger | TokenType::OctalInteger => {
                self.advance();
                let node = self.make_node_at(Operation::ConstantInteger, &tok);
                let val = parse_integer(&tok.text, tok.token_type);
                self.arena.get_mut(node).int_data[0] = val;
                self.arena.get_mut(node).nw_type = NwType::Integer;
                Ok(node)
            }

            TokenType::Float => {
                self.advance();
                let node = self.make_node_at(Operation::ConstantFloat, &tok);
                let val: f32 = tok.text.parse().unwrap_or(0.0);
                self.arena.get_mut(node).float_data = val;
                self.arena.get_mut(node).nw_type = NwType::Float;
                Ok(node)
            }

            TokenType::String => {
                self.advance();
                let node = self.make_node_at(Operation::ConstantString, &tok);
                self.arena.get_mut(node).string_data = Some(tok.text.clone());
                self.arena.get_mut(node).nw_type = NwType::String;
                Ok(node)
            }

            TokenType::KeywordObjectSelf | TokenType::KeywordObjectInvalid => {
                self.advance();
                let node = self.make_node_at(Operation::ConstantObject, &tok);
                self.arena.get_mut(node).nw_type = NwType::Object;
                self.arena.get_mut(node).int_data[0] = if tok.token_type == TokenType::KeywordObjectSelf { 0 } else { 0x7f000000u32 as i32 };
                Ok(node)
            }

            TokenType::KeywordJsonNull | TokenType::KeywordJsonFalse | TokenType::KeywordJsonTrue
            | TokenType::KeywordJsonObject | TokenType::KeywordJsonArray | TokenType::KeywordJsonString => {
                self.advance();
                let node = self.make_node_at(Operation::ConstantJson, &tok);
                self.arena.get_mut(node).int_data[0] = tok.token_type as i32;
                Ok(node)
            }

            TokenType::KeywordLocationInvalid => {
                self.advance();
                let node = self.make_node_at(Operation::ConstantLocation, &tok);
                Ok(node)
            }

            TokenType::KeywordDashDashFunction | TokenType::KeywordDashDashFile
            | TokenType::KeywordDashDashLine | TokenType::KeywordDashDashDate
            | TokenType::KeywordDashDashTime => {
                self.advance();
                match tok.token_type {
                    TokenType::KeywordDashDashLine => {
                        let node = self.make_node_at(Operation::ConstantInteger, &tok);
                        self.arena.get_mut(node).int_data[0] = tok.line as i32;
                        self.arena.get_mut(node).nw_type = NwType::Integer;
                        Ok(node)
                    }
                    TokenType::KeywordDashDashFile => {
                        let node = self.make_node_at(Operation::ConstantString, &tok);
                        self.arena.get_mut(node).nw_type = NwType::String;
                        let fname = self.file_names.first().cloned().unwrap_or_default();
                        self.arena.get_mut(node).string_data = Some(fname);
                        Ok(node)
                    }
                    TokenType::KeywordDashDashFunction => {
                        let node = self.make_node_at(Operation::ConstantString, &tok);
                        self.arena.get_mut(node).nw_type = NwType::String;
                        self.arena.get_mut(node).string_data = Some(String::new());
                        self.arena.get_mut(node).int_data[1] = 1; // marker: fill in during codegen
                        Ok(node)
                    }
                    TokenType::KeywordDashDashDate => {
                        let node = self.make_node_at(Operation::ConstantString, &tok);
                        self.arena.get_mut(node).nw_type = NwType::String;
                        self.arena.get_mut(node).string_data = Some(String::new()); // filled at compile time
                        Ok(node)
                    }
                    _ => {
                        // __TIME__
                        let node = self.make_node_at(Operation::ConstantString, &tok);
                        self.arena.get_mut(node).nw_type = NwType::String;
                        self.arena.get_mut(node).string_data = Some(String::new());
                        Ok(node)
                    }
                }
            }

            TokenType::LeftBracket => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect(TokenType::RightBracket)?;
                Ok(expr)
            }

            TokenType::KeywordVector => {
                self.advance();
                self.expect(TokenType::LeftBracket)?;
                let x = self.parse_expression()?;
                self.expect(TokenType::Comma)?;
                let y = self.parse_expression()?;
                self.expect(TokenType::Comma)?;
                let z = self.parse_expression()?;
                self.expect(TokenType::RightBracket)?;
                let node = self.make_node_at(Operation::ConstantVector, &tok);
                self.arena.get_mut(node).nw_type = NwType::Vector;
                // store as linked list: x -> y -> z
                let arg1 = self.make_node(Operation::ActionArgList);
                let arg2 = self.make_node(Operation::ActionArgList);
                self.arena.get_mut(arg1).left = x;
                self.arena.get_mut(arg1).right = arg2;
                self.arena.get_mut(arg2).left = y;
                let arg3 = self.make_node(Operation::ActionArgList);
                self.arena.get_mut(arg2).right = arg3;
                self.arena.get_mut(arg3).left = z;
                self.arena.get_mut(node).left = arg1;
                Ok(node)
            }

            TokenType::Identifier => {
                self.advance();
                if self.peek_type() == TokenType::LeftBracket {
                    self.advance();
                    let action = self.make_node_at(Operation::Action, &tok);
                    let action_id = self.make_node_at(Operation::ActionId, &tok);
                    self.arena.get_mut(action_id).string_data = Some(tok.text.clone());

                    let args = self.parse_argument_list()?;

                    self.expect(TokenType::RightBracket)?;

                    self.arena.get_mut(action).left = action_id;
                    self.arena.get_mut(action_id).right = args;
                    return Ok(action);
                }

                let node = self.make_node_at(Operation::Variable, &tok);
                self.arena.get_mut(node).string_data = Some(tok.text.clone());
                Ok(node)
            }

            _ => {
                Err(CompileError::BadStartOfStatement)
            }
        }
    }

    fn parse_argument_list(&mut self) -> Result<NodeId, CompileError> {
        if self.peek_type() == TokenType::RightBracket {
            return Ok(NULL_NODE);
        }

        let mut first = NULL_NODE;

        loop {
            let arg = self.parse_expression()?;
            let arg_node = self.make_node(Operation::ActionArgList);
            self.arena.get_mut(arg_node).left = arg;

            if first == NULL_NODE {
                first = arg_node;
            } else {
                let last = self.find_rightmost(first);
                self.arena.get_mut(last).right = arg_node;
            }

            if self.peek_type() != TokenType::Comma {
                break;
            }
            self.advance();
        }

        Ok(first)
    }
}

fn parse_integer(text: &str, tt: TokenType) -> i32 {
    match tt {
        TokenType::HexInteger => {
            let hex = text.trim_start_matches("0x").trim_start_matches("0X");
            i64::from_str_radix(hex, 16).unwrap_or(0) as i32
        }
        TokenType::BinaryInteger => {
            let bin = text.trim_start_matches("0b").trim_start_matches("0B");
            i64::from_str_radix(bin, 2).unwrap_or(0) as i32
        }
        TokenType::OctalInteger => {
            let oct = text.trim_start_matches("0o").trim_start_matches("0O");
            i64::from_str_radix(oct, 8).unwrap_or(0) as i32
        }
        _ => text.parse::<i64>().unwrap_or(0) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Parser {
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push("test.nss".to_string());
        let _ = parser.parse_program();
        parser
    }

    #[test]
    fn test_empty_function() {
        let p = parse("void main() { }");
        assert!(p.diagnostics.is_empty());
        assert!(!p.arena.is_empty());
    }

    #[test]
    fn test_function_with_return() {
        let p = parse("int foo() { return 42; }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_global_variable() {
        let p = parse("int gMyVar = 10;");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_struct_definition() {
        let p = parse("struct MyStruct { int x; float y; string name; };");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_if_else() {
        let p = parse("void main() { if (1) { return; } else { return; } }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_while_loop() {
        let p = parse("void main() { while (1) { break; } }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_for_loop() {
        let p = parse("void main() { for (int i = 0; i < 10; i++) { } }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_function_call() {
        let p = parse("void main() { foo(1, 2, 3); }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_expressions() {
        let p = parse("void main() { int x = 1 + 2 * 3 - 4 / 5; }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_multi_error_recovery() {
        let mut lexer = Lexer::new(
            "void foo() { int x = ; } void bar() { int y = ; }",
            "test.nss",
            0,
        );
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push("test.nss".to_string());
        parser.set_collect_all_errors(true);
        let _ = parser.parse_program();
        assert!(parser.diagnostics.len() >= 2);
    }

    #[test]
    fn test_const_global() {
        let p = parse("const int MY_CONST = 42;");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_ternary() {
        let p = parse("void main() { int x = (1 > 0) ? 1 : 0; }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_switch() {
        let src = r#"
        void main() {
            int x = 1;
            switch (x) {
                case 1: break;
                case 2: break;
                default: break;
            }
        }
        "#;
        let p = parse(src);
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_do_while() {
        let p = parse("void main() { do { break; } while (1); }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_function_with_params() {
        let p = parse("void foo(int a, float b, string c) { }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_function_declaration() {
        let p = parse("void foo(int a, float b);");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_include() {
        let p = parse(r#"#include "mylib""#);
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_optional_params() {
        let p = parse("void foo(int a, int b = 10, string c = \"hello\");");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_struct_field_access() {
        let p = parse("void main() { int x = myStruct.field; }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_negation() {
        let p = parse("void main() { int x = -1; }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_boolean_not() {
        let p = parse("void main() { int x = !0; }");
        assert!(p.diagnostics.is_empty());
    }

    #[test]
    fn test_pre_post_increment() {
        let p = parse("void main() { int x = 0; x++; ++x; x--; --x; }");
        assert!(p.diagnostics.is_empty());
    }
}
