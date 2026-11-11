use std::collections::HashMap;
use slotmap::{SparseSecondaryMap};

use ::analysis::{symbol_table::{SymbolTable, Declaration, DeclarationType}, typecheck::TypeCast};
use lex_parse::{ast::{AST, ASTNodeHandle, ASTNode, BinaryOpType, UnaryOpType, ASTNodePrintable}, types::StorageQual};
use analysis::{analysis};
use crate::asmprinter::{AsmPrinter, Register, LC3Bundle, LC3Bundle::*, LC3Inst, Immediate as Imm, Label, LC3Directive};
use lex_parse::context::{Context, InternedType, InternedString};


pub struct Codegen<'a> {
    // State
    regfile: [bool; 8],
    scope: InternedString,

    // 常量池：立即数放不进 imm5 时走 LD，值 → 标号
    const_pool: HashMap<i32, String>,
    // 程序中用到了 '*'，结尾需要追加 __mul 运行时
    used_mul: bool,

    // External Information
    symbol_table: SymbolTable,
    printer: &'a mut AsmPrinter,
    ast: &'a AST,
    context: &'a Context<'a>,
    casts: SparseSecondaryMap<ASTNodeHandle, TypeCast>,

    // Random trash:
    if_counter: usize,
    while_counter: usize,
    for_counter: usize,
    // Suki 兼容模式下布尔归一化序列的标号计数
    bool_counter: usize,
}

#[macro_use]
macro_rules! emit {
    ($self:ident, $expression:expr) => {
        $self.printer.inst($expression);
    }
}

impl<'a> Codegen<'a> {
    const USED: bool = true;
    const UNUSED: bool = false;
    const R5: Register = Register{value: 5};
    const R6: Register = Register{value: 6};
    const R7: Register = Register{value: 7};

    pub fn new(ast: &'a AST, printer: &'a mut AsmPrinter, context: &'a Context<'a>, symbol_table: SymbolTable, casts: SparseSecondaryMap<ASTNodeHandle, TypeCast>) -> Codegen<'a> {
        Codegen { regfile: [false ; 8], context, symbol_table, printer, ast, casts, scope: context.get_string("global"),
        const_pool: HashMap::new(), used_mul: false,
        if_counter: 0, while_counter: 0, for_counter: 0, bool_counter: 0}
    }

    fn get_empty_reg(&self) -> Register {
        // TODO: Flag
        for n in 0..4 {
            if self.regfile[n] == false {
                return Register {value: n};
            }
        }
        println!("error: cannot allocate registers effectively, please move complex expressions to separate statements.");
        panic!(); // TODO: Surpress panic.
    }

    fn reset_regfile(&mut self) -> () {
        for n in 0..4 {
            self.regfile[n] = false;
        }
    }

    // 标量变量（全局/静态局部/普通局部）的存放位置
    // 返回 Some(标号) 表示经标号访问；None 表示 R5 相对的栈变量
    fn scalar_storage(&self, entry: &Declaration) -> Option<String> {
        let identifier = self.context.resolve_string(entry.identifier);
        if entry.is_global {
            return Some(identifier);
        }
        if self.context.resolve_type(entry.type_info).specifier.qualifiers.storage == StorageQual::Static {
            let function = self.context.resolve_string(self.scope);
            return Some(format!("{function}.{identifier}"));
        }
        None
    }

    // 把常数装进寄存器：小立即数用 AND/ADD，大数走常量池 LD
    fn emit_load_const(&mut self, reg: Register, value: i32, comment: Option<String>) -> () {
        if value == 0 {
            emit!(self, Instruction(LC3Inst::AndImm(reg, reg, Imm::Int(0)), comment));
            return;
        }
        if (-16..=15).contains(&value) {
            emit!(self, Instruction(LC3Inst::AndImm(reg, reg, Imm::Int(0)), None));
            emit!(self, Instruction(LC3Inst::AddImm(reg, reg, Imm::Int(value)), comment));
            return;
        }
        let label = self.const_label(value);
        emit!(self, Instruction(LC3Inst::Ld(reg, Label::Label(label)), comment));
    }

    fn const_label(&mut self, value: i32) -> String {
        if let Some(label) = self.const_pool.get(&value) {
            return label.clone();
        }
        let label = if value < 0 {
            format!("CONST_M{}", (value as i64).unsigned_abs())
        } else {
            format!("CONST_{}", value)
        };
        self.const_pool.insert(value, label.clone());
        label
    }

    // reg += imm，保证立即数合法（超范围时借临时寄存器）
    fn emit_add_imm(&mut self, reg: Register, imm: i32) -> () {
        if (-16..=15).contains(&imm) {
            emit!(self, Instruction(LC3Inst::AddImm(reg, reg, Imm::Int(imm)), None));
            return;
        }
        let saved = self.regfile[reg.value];
        self.regfile[reg.value] = Self::USED;
        let tmp = self.get_empty_reg();
        self.emit_load_const(tmp, imm, None);
        emit!(self, Instruction(LC3Inst::AddReg(reg, reg, tmp), None));
        self.regfile[tmp.value] = Self::UNUSED;
        self.regfile[reg.value] = saved;
    }

    fn unsupported(&self, node: &ASTNode, what: &str) -> ! {
        println!("error: unsupported feature: {what}");
        println!("{}", ASTNodePrintable{node: node.clone(), context: self.context});
        std::process::exit(1);
    }

    // 把表达式归一化成 0/1 布尔值（比较运算天然归一，其余按非零为真）
    fn emit_bool(&mut self, node_h: &ASTNodeHandle) -> Register {
        let node = self.ast.get_node(node_h);
        if let ASTNode::BinaryOp { op, .. } = node {
            match op {
                BinaryOpType::LessThan | BinaryOpType::GreaterThan
                | BinaryOpType::LessThanEqual | BinaryOpType::GreaterThanEqual
                | BinaryOpType::EqualEqual | BinaryOpType::NotEqual
                | BinaryOpType::LogAnd | BinaryOpType::LogOr => {
                    return self.emit_condition_node(node_h);
                }
                _ => ()
            }
        }
        let reg = self.emit_expression_node(node_h);
        self.normalize_truth(reg);
        reg
    }

    // reg = (reg != 0) ? 1 : 0，同时把条件码归一到 P/Z
    fn normalize_truth(&mut self, reg: Register) -> () {
        emit!(self, Instruction(LC3Inst::AddImm(reg, reg, Imm::Int(0)), Some("set CC from value".to_string())));
        // 非零（N 或 P）→ 1，零 → 0
        self.normalize_bool(reg, (true, false, true));
    }

    // 统一的布尔归一化：条件码满足 cc 时取 1，否则取 0
    // 默认（lc3tools）：BRcc #2 / AND ret,#0 / BRnzp #2 / AND ret,#0 / ADD ret,#1
    // Suki 兼容模式：数字偏移改写为标号
    fn normalize_bool(&mut self, ret: Register, cc: (bool, bool, bool)) -> () {
        if !self.printer.suki_compat() {
            emit!(self, Instruction(LC3Inst::BrImm(cc.0, cc.1, cc.2, Imm::Int(2)), None));
            emit!(self, Instruction(LC3Inst::AndImm(ret, ret, Imm::Int(0)), Some("evaluate to '0' or 'false'".to_string())));
            emit!(self, Instruction(LC3Inst::BrImm(true, true, true, Imm::Int(2)), None));
            emit!(self, Instruction(LC3Inst::AndImm(ret, ret, Imm::Int(0)), None));
            emit!(self, Instruction(LC3Inst::AddImm(ret, ret, Imm::Int(1)), Some("evaluate to '1' or 'true'".to_string())));
            return;
        }

        let func = self.context.resolve_string(self.scope);
        let n = self.bool_counter;
        self.bool_counter += 1;
        let label_true = Label::Label(format!("{func}.bool.{n}.true"));
        let label_end = Label::Label(format!("{func}.bool.{n}.end"));

        emit!(self, Instruction(LC3Inst::Br(cc.0, cc.1, cc.2, label_true.clone()), None));
        emit!(self, Instruction(LC3Inst::AndImm(ret, ret, Imm::Int(0)), Some("evaluate to '0' or 'false'".to_string())));
        emit!(self, Instruction(LC3Inst::Br(true, true, true, label_end.clone()), None));
        emit!(self, HeaderLabel(label_true, None));
        emit!(self, Instruction(LC3Inst::AndImm(ret, ret, Imm::Int(0)), None));
        emit!(self, Instruction(LC3Inst::AddImm(ret, ret, Imm::Int(1)), Some("evaluate to '1' or 'true'".to_string())));
        emit!(self, HeaderLabel(label_end, None));
    }

    fn emit_condition_node(&mut self, node_h: &ASTNodeHandle) -> Register {
        let node = self.ast.get_node(node_h);

        let (op, left, right) = match node {
            ASTNode::BinaryOp{op, left, right} => (op, left, right),
            _ => panic!()
        };

        // 逻辑与/或：两边先各自归一化成 0/1，再按位组合
        match op {
            BinaryOpType::LogAnd => {
                let right_reg = self.emit_bool(right);
                self.regfile[right_reg.value] = Self::USED;
                let left_reg = self.emit_bool(left);
                emit!(self, Instruction(LC3Inst::AndReg(left_reg, left_reg, right_reg), Some("evaluate '&&'".to_string())));
                self.regfile[right_reg.value] = Self::UNUSED;
                return left_reg;
            }
            BinaryOpType::LogOr => {
                // a || b = ~(~a & ~b)，LC-3 没有按位或
                let right_reg = self.emit_bool(right);
                self.regfile[right_reg.value] = Self::USED;
                let left_reg = self.emit_bool(left);
                emit!(self, Instruction(LC3Inst::Not(left_reg, left_reg), Some("evaluate '||'".to_string())));
                emit!(self, Instruction(LC3Inst::Not(right_reg, right_reg), None));
                emit!(self, Instruction(LC3Inst::AndReg(left_reg, left_reg, right_reg), None));
                emit!(self, Instruction(LC3Inst::Not(left_reg, left_reg), None));
                self.regfile[right_reg.value] = Self::UNUSED;
                return left_reg;
            }
            _ => ()
        }

        let right = self.emit_expression_node(right);
        self.regfile[right.value] = Self::USED;
        let left = self.emit_expression_node(left);

        let ret = left;

        match op {
            BinaryOpType::LessThan => {
                // ret = right - left，为正即 left < right
                emit!(self, Instruction(LC3Inst::Not(ret, ret), Some("evaluate '<'".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(ret, ret, Imm::Int(1)), None));
                emit!(self, Instruction(LC3Inst::AddReg(ret, ret, right), None));
                self.normalize_bool(ret, (false, false, true)); // BRp
            },
            BinaryOpType::GreaterThan => {
                // ret = left - right，为正即 left > right
                emit!(self, Instruction(LC3Inst::Not(right, right), Some("evaluate '>'".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(right, right, Imm::Int(1)), None));
                emit!(self, Instruction(LC3Inst::AddReg(ret, left, right), None));
                self.normalize_bool(ret, (false, false, true)); // BRp
            },
            BinaryOpType::LessThanEqual => {
                // ret = left - right，非负即 left <= right
                emit!(self, Instruction(LC3Inst::Not(right, right), Some("evaluate '<='".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(right, right, Imm::Int(1)), None));
                emit!(self, Instruction(LC3Inst::AddReg(ret, left, right), None));
                self.normalize_bool(ret, (false, true, true)); // BRzp
            },
            BinaryOpType::GreaterThanEqual => {
                // ret = right - left，非负即 left >= right
                emit!(self, Instruction(LC3Inst::Not(ret, ret), Some("evaluate '>='".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(ret, ret, Imm::Int(1)), None));
                emit!(self, Instruction(LC3Inst::AddReg(ret, ret, right), None));
                self.normalize_bool(ret, (false, true, true)); // BRzp
            },
            BinaryOpType::EqualEqual => {
                // ret = left - right，为零即相等
                emit!(self, Instruction(LC3Inst::Not(right, right), Some("evaluate '=='".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(right, right, Imm::Int(1)), None));
                emit!(self, Instruction(LC3Inst::AddReg(ret, left, right), None));
                self.normalize_bool(ret, (false, true, false)); // BRz
            }
            BinaryOpType::NotEqual => {
                // ret = left - right，非零即不等
                emit!(self, Instruction(LC3Inst::Not(right, right), Some("evaluate '!='".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(right, right, Imm::Int(1)), None));
                emit!(self, Instruction(LC3Inst::AddReg(ret, left, right), None));
                self.normalize_bool(ret, (true, false, true)); // BRnp
            }
            _ => {
                self.unsupported(node, "condition operator");
            }
        }

        self.regfile[right.value] = Self::UNUSED;

        return ret;
    }


    pub fn emit_ast_node(&mut self, node_h: &ASTNodeHandle) {
        // If counter, while_counter, for_counter etc..
        let node = self.ast.get_node(node_h);
        match node {
            ASTNode::Program { declarations } => {

                emit!(self, Instruction(LC3Inst::Ld(Self::R6, Label::Label("USER_STACK".to_string())), None));
                emit!(self, Instruction(LC3Inst::AddImm(Self::R5, Self::R6, Imm::Int(-1)), None));
                emit!(self, Instruction(LC3Inst::Jsr(Label::Label("main".to_string())), None));
                // main 正常情况下直接 HALT，这里兜底防误入数据段
                emit!(self, Instruction(LC3Inst::Halt, Some("safety halt".to_string())));

                emit!(self, Newline);
                for decl in declarations {
                    self.emit_ast_node(decl);
                }

                if self.used_mul {
                    self.emit_mul_runtime();
                }

                // 常量池：所有超出 imm5 的立即数，统一放数据段（按值排序，保证输出确定）
                let mut pool: Vec<(&i32, &String)> = self.const_pool.iter().collect();
                pool.sort_by_key(|(value, _)| **value);
                for (value, label) in pool {
                    self.printer.data(Directive(Some(Label::Label(label.clone())), LC3Directive::Fill(*value), Some("constant pool".to_string())));
                }
            }
            // Decls:
            ASTNode::FunctionDecl { body, parameters, identifier, return_type } => {
                emit!(self, Newline);
                self.scope = *identifier;

                let function = self.context.resolve_string(self.scope);
                let identifier = self.context.resolve_string(*identifier);

                let frame_size = self.symbol_table.frame_sizes.get(*node_h).copied().unwrap_or(0);

                if function == "main" {
                    emit!(self, HeaderLabel(Label::Label(identifier.clone()), None));
                    // main 无 callee setup，帧指针已在序言设好，这里一次性分配局部变量
                    if frame_size > 0 {
                        self.emit_add_imm(Self::R6, -frame_size);
                    }
                    self.emit_ast_node(body);
                    // 落出函数尾或 `return;` 时也要停机
                    emit!(self, Instruction(LC3Inst::Halt, Some("halt at end of main".to_string())));
                    return;
                }

                emit!(self, HeaderLabel(Label::Label(identifier.clone()), None));
                // TODO: Support 'builder' syntax for these instructions. The entire first 20 chars really should not be necessary.
                emit!(self, SectionComment("callee setup:".to_string()));
                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), Some("allocate spot for return value".to_string())));

                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
                emit!(self, Instruction(LC3Inst::Str(Self::R7, Self::R6, Imm::Int(0)), Some("push R7 (return address)".to_string())));

                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
                emit!(self, Instruction(LC3Inst::Str(Self::R5, Self::R6, Imm::Int(0)), Some("push R5 (caller frame pointer)".to_string())));

                emit!(self, Instruction(LC3Inst::AddImm(Self::R5, Self::R6, Imm::Int(-1)), Some("set frame pointer".to_string())));

                // 序言一次性分配全部局部变量，循环体里不再重复分配
                if frame_size > 0 {
                    self.emit_add_imm(Self::R6, -frame_size);
                }

                emit!(self, Newline);
                emit!(self, SectionComment("function body:".to_string()));
                self.emit_ast_node(body);

                let teardown_label = format!("{identifier}.teardown").to_string();

                emit!(self, HeaderLabel(Label::Label(teardown_label), None));

                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R5, Imm::Int(1)), Some("pop local variables".to_string())));

                emit!(self, Instruction(LC3Inst::Ldr(Self::R5, Self::R6, Imm::Int(0)), Some("pop frame pointer".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(1)), None));

                emit!(self, Instruction(LC3Inst::Ldr(Self::R7, Self::R6, Imm::Int(0)), Some("pop return address".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(1)), None));

                emit!(self, Instruction(LC3Inst::Ret, None));
                emit!(self, SectionComment("end function.".to_string()));
                emit!(self, Newline);
            },
            ASTNode::ParameterDecl { identifier, type_info } => {},
            ASTNode::VariableDecl { identifier, initializer, type_info } => {

                let identifier = self.context.resolve_string(*identifier);


                let entry = self.symbol_table.entries.get(*node_h).unwrap();
                // Global Variable
                if entry.is_global {
                    let value = match initializer {
                        Some(initializer) => {
                            // Need to do constant evaluation.
                            if let ASTNode::IntLiteral { value } = self.ast.get_node(initializer) {
                                *value
                            }
                            else {0}
                        }
                        None => 0
                    };

                    self.printer.data(Directive(Some(Label::Label(identifier)), LC3Directive::Fill(value), None));
                    return;
                }

                let function = self.context.resolve_string(self.scope);
                // Static Local Variable
                if self.context.resolve_type(entry.type_info).specifier.qualifiers.storage == StorageQual::Static {
                    let full_identifier = format!("{function}.{identifier}");
                    // TODO: Constant evaluation of expressions
                    let value = match initializer {
                        Some(initializer) => {
                            // Need to do constant evaluation.
                            if let ASTNode::IntLiteral { value } = self.ast.get_node(initializer) {
                                *value
                            }
                            else {0}
                        }
                        None => 0
                    };
                    self.printer.data(Directive(Some(Label::Label(full_identifier)), LC3Directive::Fill(value), None));
                }

                // Nonstatic local variable
                else {
                    // 空间已在序言一次分配，这里只做初始化
                    match initializer {
                        Some(initializer) => {
                            // Need to do constant evaluation.
                            let reg = self.emit_expression_node(initializer);
                            let entry = self.symbol_table.entries.get(*node_h).unwrap();
                            self.regfile[reg.value] = Self::UNUSED;

                            emit!(self, Instruction(LC3Inst::Str(reg, Self::R5, Imm::Int(entry.offset)), Some(format!("initialize '{identifier}'"))));
                        }
                        None => ()
                    }
                }
            },

            // Statements:
            ASTNode::Ternary { first, second, third } => {
                self.unsupported(node, "ternary operator '?:'");
            },
            ASTNode::CompoundStmt { statements, new_scope } => {
                for stmt in statements {
                    self.emit_ast_node(stmt);
                    emit!(self, Newline);
                    self.reset_regfile();
                }
            },
            ASTNode::ExpressionStmt { expression } => {
                let reg = self.emit_expression_node(expression);
                self.regfile[reg.value] = Self::UNUSED;
            },
            ASTNode::ReturnStmt { expression } => {
                let function = self.context.resolve_string(self.scope);

                if function == "main" {
                    match expression {
                        Some(expression) => {
                            let reg = self.emit_expression_node(expression);
                            emit!(self, Instruction(LC3Inst::Sti(reg, Label::Label("RETURN_SLOT".to_string())), Some("write return value from main".to_string())));
                            self.regfile[reg.value] = Self::UNUSED;
                        },
                        None =>  ()
                    }
                    emit!(self, Instruction(LC3Inst::Halt, None));
                }
                else {
                    match expression {
                        Some(expression) => {
                            let reg = self.emit_expression_node(expression);
                            emit!(self, Instruction(LC3Inst::Str(reg, Self::R5, Imm::Int(3)), Some("write return value, always R5 + 3".to_string())));
                            self.regfile[reg.value] = Self::UNUSED;
                        },
                        None =>  ()
                    }

                    let teardown_label = format!("{function}.teardown");

                    emit!(self, Instruction(LC3Inst::Br(true, true, true, Label::Label(teardown_label)), None));
                }

                // Maybe we do "RETURN slot"

            }
            ASTNode::ForStmt { initializer, condition, update, body } => {
                emit!(self, SectionComment("for loop initialization".to_string()));

                let func_name = self.context.resolve_string(self.scope);

                let name = format!("{func_name}.for.{}", self.for_counter);
                // 立即占用编号，保证嵌套循环标号唯一
                self.for_counter += 1;
                let label_header = Label::Label(format!("{name}"));
                let label_end = Label::Label(format!("{name}.end"));

                self.emit_ast_node(initializer);
                self.reset_regfile();

                emit!(self, HeaderLabel(label_header.clone(), Some("test condition".to_string())));

                let condition = self.emit_bool(condition);
                self.reset_regfile();
                //emit!(self, Instruction(LC3Inst::AndReg(condition, condition, condition), Some("load condition into NZP".to_string())));

                emit!(self, Instruction(LC3Inst::Br(true, true, false, label_end.clone()), Some("if false, skip over loop body".to_string())));

                self.emit_ast_node(body);

                emit!(self, SectionComment("update expression".to_string()));
                self.emit_ast_node(update);
                self.reset_regfile();

                emit!(self, Instruction(LC3Inst::Br(true, true, true, label_header), Some("loop".to_string())));

                emit!(self, HeaderLabel(label_end, None));
            }


            ASTNode::WhileStmt { condition, body } => {


                let func_name = self.context.resolve_string(self.scope);
                let name = format!("{func_name}.while.{}", self.while_counter);
                // 立即占用编号，保证嵌套循环标号唯一
                self.while_counter += 1;
                let label_header = Label::Label(format!("{name}"));
                let label_end = Label::Label(format!("{name}.end"));

                emit!(self, Newline);
                emit!(self, HeaderLabel(label_header.clone(), Some("while loop begin".to_string())));
                let condition = self.emit_bool(condition);
                self.reset_regfile();
                // 条件为假（0 → CC=Z）跳出；与 for 的出口极性一致
                emit!(self, Instruction(LC3Inst::Br(true, true, false, label_end.clone()), Some("if false, skip to end".to_string())));
                self.emit_ast_node(body);
                //emit!(self, Instruction(LC3Inst::AndReg(condition, condition, condition), Some("load condition into NZP".to_string())));
                emit!(self, Instruction(LC3Inst::Br(true, true, true, label_header), Some("test loop condition".to_string())));
                emit!(self, HeaderLabel(label_end, Some("while loop end".to_string())));
            }

            //ASTNode::DeclStmt { declarations } => todo!(),
            //ASTNode::InlineAsm { assembly } => todo!(),

            ASTNode::IfStmt { condition, if_branch, else_branch } => {
                // TOOD: If this is a simple condition, then we don't need to load the condition into NZP.
                let condition = self.emit_bool(condition);
                self.reset_regfile();
                emit!(self, Newline);
                //emit!(self, Instruction(LC3Inst::AndReg(condition, condition, condition), Some("load condition into NZP".to_string())));

                let func_name = self.context.resolve_string(self.scope);

                let name = format!("{func_name}.if.{}", self.if_counter);
                // 立即占用编号，保证嵌套 if 标号唯一
                self.if_counter += 1;
                let label_header = Label::Label(format!("{name}"));
                let label_end = Label::Label(format!("{name}.end"));
                let label_else: Label = Label::Label(format!("{name}.else"));

                match else_branch {
                    Some(else_branch) => {
                        emit!(self, Instruction(LC3Inst::Br(true, true, false, label_else.clone()), Some("if false, jump to else block".to_string())));
                        emit!(self, Newline);
                        self.emit_ast_node(if_branch);

                        emit!(self, Instruction(LC3Inst::Br(true, true, true, label_end.clone()), None));
                        emit!(self, HeaderLabel(label_else, None));
                        emit!(self, Newline);

                        self.emit_ast_node(else_branch);
                        emit!(self, HeaderLabel(label_end, None));
                    }
                    None => {
                        emit!(self, Instruction(LC3Inst::Br(true, true, false, label_end.clone()), Some("if false, jump over if statement".to_string())));
                        emit!(self, Newline);
                        self.emit_ast_node(if_branch);
                        emit!(self, HeaderLabel(label_end, None));
                    }
                }

            },
            ASTNode::RecordDecl { identifier, record_type, fields } => {
                return;
            },
            ASTNode::FieldDecl { identifier, type_info } => {
                return;
            },

            // Expressions:
            ASTNode::IntLiteral { value } => {self.emit_expression_node(node_h); ()},
            ASTNode::FunctionCall { symbol_ref, arguments } => {self.emit_expression_node(node_h); ()},
            ASTNode::SymbolRef { identifier } => {self.emit_expression_node(node_h); ()},
            ASTNode::BinaryOp { op, right, left } => {self.emit_expression_node(node_h); ()},
            ASTNode::UnaryOp { op, child, order } => {self.emit_expression_node(node_h); ()},
            _ => {
                self.unsupported(node, "statement");
            }
        }

    }

    fn emit_expression_node(&mut self, node_h: &ASTNodeHandle) -> Register {

        let node = self.ast.get_node(node_h);

        match node {
            ASTNode::BinaryOp { op, right, left } => {
                let left_node = self.ast.get_node(left);
                let right_node = self.ast.get_node(right);

                match op {
                    // Lots of annoying stuff here, be prepared
                    BinaryOpType::Assign => {
                        // Need to check sizes?? of lhs and rhs to know how many assignments to emit. WTf.

                        // Handle RHS first:
                        let rhs = self.emit_expression_node(right);
                        self.regfile[rhs.value] = Self::USED;

                        // Handle LHS now

                        // OPTIMIZATION:
                        if let ASTNode::SymbolRef { identifier } = left_node {
                            let entry = self.symbol_table.entries.get(*left).unwrap();

                            let label = self.scalar_storage(entry);

                            if let Some(label) = label {
                                emit!(self, Instruction(
                                    LC3Inst::St(rhs, Label::Label(label)), Some("assign to variable".to_string())));
                            } else {
                                emit!(self, Instruction(
                                    LC3Inst::Str(rhs, Self::R5, Imm::Int(entry.offset)), Some(format!("assign to variable {}", self.context.resolve_string(entry.identifier)))));
                            }
                        }
                        else {
                            // In general, lhs should evaluate to an adrress.
                            let lhs = self.emit_expression_node(left);
                            // Treat LHS as address
                            emit!(self, Instruction(
                                LC3Inst::Str(rhs, lhs, Imm::Int(0)), None)); // Store RHS into LHS as address
                        }

                        rhs

                    }
                    BinaryOpType::Add => {
                        if let ASTNode::IntLiteral { value } = left_node {
                            if (-16..=15).contains(value) {
                                let reg = self.emit_expression_node(right);
                                self.emit_add_imm(reg, *value);
                                self.regfile[reg.value] = Self::USED;
                                return reg;
                            }
                        }
                        else if let ASTNode::IntLiteral { value } = right_node {
                            if (-16..=15).contains(value) {
                                let reg = self.emit_expression_node(left);
                                self.emit_add_imm(reg, *value);
                                self.regfile[reg.value] = Self::USED;
                                return reg;
                            }
                        }

                        let left = self.emit_expression_node(left);
                        self.regfile[left.value] = Self::USED;
                        let right = self.emit_expression_node(right);

                        emit!(self, Instruction(
                            LC3Inst::AddReg(left, left, right), None));

                        self.regfile[right.value] = Self::UNUSED;
                        return left;

                    }
                    BinaryOpType::Sub => {
                        if let ASTNode::IntLiteral { value } = right_node {
                            let reg = self.emit_expression_node(left);
                            self.regfile[reg.value] = Self::USED;

                            self.emit_add_imm(reg, -1 * value);

                            return reg;
                        }
                        else {
                            let left = self.emit_expression_node(left);
                            self.regfile[left.value] = Self::USED;
                            let right = self.emit_expression_node(right);
                            emit!(self, Instruction(LC3Inst::Not(right, right), None));
                            emit!(self, Instruction(LC3Inst::AddImm(right, right, Imm::Int(1)), None));
                            emit!(self, Instruction(LC3Inst::AddReg(left, right, left), None));
                            self.regfile[right.value] = Self::UNUSED;

                            return left;
                        }
                    }
                    BinaryOpType::Mul => {
                        // 借助 __mul 运行时（重复加法）实现
                        self.used_mul = true;

                        // caller-save：把当前活跃寄存器压栈
                        let mut caller_saved: Vec<Register> = Vec::new();
                        for n in 0..7 {
                            if self.regfile[n] == true {
                                let reg = Register {value: n};
                                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
                                emit!(self, Instruction(LC3Inst::Str(reg, Self::R6, Imm::Int(0)), Some(format!("caller save {reg}"))));
                                caller_saved.push(reg);
                                emit!(self, Newline);
                            }
                        }

                        // 先右后左压栈，使左操作数位于栈顶（__mul 的 a）
                        let right_reg = self.emit_expression_node(right);
                        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
                        emit!(self, Instruction(LC3Inst::Str(right_reg, Self::R6, Imm::Int(0)), Some("push operand".to_string())));
                        self.regfile[right_reg.value] = Self::UNUSED;
                        emit!(self, Newline);

                        let left_reg = self.emit_expression_node(left);
                        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
                        emit!(self, Instruction(LC3Inst::Str(left_reg, Self::R6, Imm::Int(0)), Some("push operand".to_string())));
                        self.regfile[left_reg.value] = Self::UNUSED;
                        emit!(self, Newline);

                        emit!(self, Instruction(LC3Inst::Jsr(Label::Label("__mul".to_string())), Some("call __mul".to_string())));
                        emit!(self, Newline);

                        let ret = self.get_empty_reg();
                        emit!(self, Instruction(LC3Inst::Ldr(ret, Self::R6, Imm::Int(0)), Some("load return value.".to_string())));
                        // 弹返回值槽 + 两个操作数
                        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(3)), None));

                        for reg in caller_saved.into_iter().rev() {
                            emit!(self, Newline);
                            emit!(self, Instruction(LC3Inst::Ldr(reg, Self::R6, Imm::Int(0)), Some(format!("caller restore {reg}"))));
                            emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(1)), None));
                            emit!(self, Newline);
                        }
                        emit!(self, Newline);
                        return ret;
                    }
                    BinaryOpType::BitAnd => {
                        let right_reg = self.emit_expression_node(right);
                        self.regfile[right_reg.value] = Self::USED;
                        let left_reg = self.emit_expression_node(left);
                        emit!(self, Instruction(LC3Inst::AndReg(left_reg, left_reg, right_reg), Some("evaluate '&'".to_string())));
                        self.regfile[right_reg.value] = Self::UNUSED;
                        return left_reg;
                    }
                    BinaryOpType::BitOr => {
                        // a | b = ~(~a & ~b)，LC-3 没有按位或指令
                        let right_reg = self.emit_expression_node(right);
                        self.regfile[right_reg.value] = Self::USED;
                        let left_reg = self.emit_expression_node(left);
                        emit!(self, Instruction(LC3Inst::Not(left_reg, left_reg), Some("evaluate '|'".to_string())));
                        emit!(self, Instruction(LC3Inst::Not(right_reg, right_reg), None));
                        emit!(self, Instruction(LC3Inst::AndReg(left_reg, left_reg, right_reg), None));
                        emit!(self, Instruction(LC3Inst::Not(left_reg, left_reg), None));
                        self.regfile[right_reg.value] = Self::UNUSED;
                        return left_reg;
                    }

                    // Div/Mod 需要带符号商语义，暂不支持
                    BinaryOpType::Div | BinaryOpType::Mod => {
                        self.unsupported(node, "division / modulo");
                    }

                    // Conditionals
                    BinaryOpType::LogAnd |
                    BinaryOpType::LogOr |
                    BinaryOpType::LessThan |
                    BinaryOpType::GreaterThan |
                    BinaryOpType::LessThanEqual |
                    BinaryOpType::GreaterThanEqual |
                    BinaryOpType::NotEqual |
                    BinaryOpType::EqualEqual => return self.emit_condition_node(node_h),

                    // Pointer Access
                    BinaryOpType::ArrayAccess => {
                        let base = self.emit_expression_node(left);
                        self.regfile[base.value] = Self::USED;
                        let offset = self.emit_expression_node(right);
                        self.regfile[offset.value] = Self::USED;

                        let reg = self.get_empty_reg();

                        emit!(self, Instruction(LC3Inst::AddReg(reg, base, offset), Some("calculate index into array".to_string())));

                        // Only load if this needs to be an Rvalue, else we just want the address.
                        if self.casts.get(*node_h) == Some(&TypeCast::LvalueToRvalue) {
                            emit!(self, Instruction(LC3Inst::Ldr(reg, reg, Imm::Int(0)), Some("load element from array".to_string())));
                        }

                        self.regfile[base.value] = Self::UNUSED;
                        self.regfile[offset.value] = Self::UNUSED;
                        self.regfile[reg.value] = Self::USED;
                        return reg;

                    }
                    BinaryOpType::DotAccess => {
                        let base = self.emit_expression_node(left);
                        self.regfile[base.value] = Self::USED;
                        // Dereference left side.

                        // Calculate offset
                        let (field_offset, member_name) = {
                            let entry = self.symbol_table.entries.get(*right).unwrap();
                            (entry.offset, entry.identifier)
                        };

                        let reg = self.get_empty_reg();

                        // reg = base + offset（emit_add_imm 是就地加法，先拷贝基址）
                        emit!(self, Instruction(LC3Inst::AddImm(reg, base, Imm::Int(0)), Some("copy struct base".to_string())));
                        self.emit_add_imm(reg, field_offset);
                        self.regfile[reg.value] = Self::USED;

                        let member_name = self.context.resolve_string(member_name);
                        // Only load if this needs to be an Rvalue, else we just want the address.
                        if self.casts.get(*node_h) == Some(&TypeCast::LvalueToRvalue) {
                            emit!(self, Instruction(LC3Inst::Ldr(reg, reg, Imm::Int(0)), Some(format!("load element {member_name} from struct"))));
                        }

                        self.regfile[base.value] = Self::UNUSED;
                        self.regfile[reg.value] = Self::USED;
                        return reg;
                    }
                    BinaryOpType::PointerAccess => {
                        let base = self.emit_expression_node(left);
                        self.regfile[base.value] = Self::USED;
                        // Dereference left side.
                        emit!(self, Instruction(LC3Inst::Ldr(base, base, Imm::Int(0)), Some("dereference struct pointer".to_string())));

                        // Calculate offset
                        let (field_offset, member_name) = {
                            let entry = self.symbol_table.entries.get(*right).unwrap();
                            (entry.offset, entry.identifier)
                        };

                        let reg = self.get_empty_reg();

                        // reg = base + offset（emit_add_imm 是就地加法，先拷贝基址）
                        emit!(self, Instruction(LC3Inst::AddImm(reg, base, Imm::Int(0)), Some("copy struct base".to_string())));
                        self.emit_add_imm(reg, field_offset);
                        self.regfile[reg.value] = Self::USED;

                        let member_name = self.context.resolve_string(member_name);
                        // Only load if this needs to be an Rvalue, else we just want the address.
                        if self.casts.get(*node_h) == Some(&TypeCast::LvalueToRvalue) {
                            emit!(self, Instruction(LC3Inst::Ldr(reg, reg, Imm::Int(0)), Some(format!("load element {member_name} from struct"))));
                        }

                        self.regfile[base.value] = Self::UNUSED;
                        self.regfile[reg.value] = Self::USED;
                        return reg;
                    }
                    _ => {
                        self.unsupported(node, "binary operator");
                    }
                }
            },
            ASTNode::UnaryOp { op, child, order } => {
                match op {
                    UnaryOpType::Address => {
                        let child_node = self.ast.get_node(child);
                        // Assert that it is an lvalue. (semant checks this)
                        let entry = self.symbol_table.entries.get(*child).unwrap();
                        let reg = self.get_empty_reg();
                        let identifier = self.context.resolve_string(entry.identifier);

                        let label = self.scalar_storage(entry);
                        if let Some(label) = label {
                            emit!(self, Instruction(LC3Inst::Lea(reg, Label::Label(label)),
                                Some(format!("take address of '{identifier}'"))));
                        }
                        else {
                            emit!(self, Instruction(LC3Inst::AddImm(reg, Self::R5, Imm::Int(entry.offset)),
                                Some(format!("take address of '{identifier}'"))));
                        }
                        self.regfile[reg.value] = Self::USED;
                        return reg;
                    }
                    UnaryOpType::Dereference => { // Dereference alwqasy results in an L-value.
                        // Treat whatever is being dereferenced as a memory address, and just load from the address.

                        let reg = self.emit_expression_node(child);

                        // TBH i don't know why this works.
                        if self.casts.get(*node_h) == Some(&TypeCast::LvalueToRvalue) {
                            emit!(self, Instruction(LC3Inst::Ldr(reg, reg, Imm::Int(0)), Some("dereference".to_string())));
                        }
                        //
                        return reg;
                    }
                    UnaryOpType::Negate => {
                        let reg: Register = self.emit_expression_node(child);
                        emit!(self, Instruction(LC3Inst::Not(reg, reg), None));
                        emit!(self, Instruction(LC3Inst::AddImm(reg, reg, Imm::Int(1)), None));
                        return reg;
                    }
                    UnaryOpType::Positive => {
                        return self.emit_expression_node(child);
                    },
                    UnaryOpType::LogNot => {
                        // !x：x 非零得 0，x 为零得 1
                        let reg = self.emit_bool(child);
                        self.regfile[reg.value] = Self::USED;
                        emit!(self, Instruction(LC3Inst::AddImm(reg, reg, Imm::Int(0)), Some("evaluate '!'".to_string())));
                        self.normalize_bool(reg, (false, true, false)); // BRz：为零 → 1
                        self.regfile[reg.value] = Self::USED;
                        return reg;
                    }
                    UnaryOpType::BinNot => {
                        let reg = self.emit_expression_node(child);
                        emit!(self, Instruction(LC3Inst::Not(reg, reg), Some("evaluate '~'".to_string())));
                        return reg;
                    }
                    UnaryOpType::Increment | UnaryOpType::Decrement => {
                        // 子表达式按 lvalue 处理，求出地址后读-改-写
                        let addr = self.emit_expression_node(child);
                        self.regfile[addr.value] = Self::USED;

                        let old = self.get_empty_reg();
                        emit!(self, Instruction(LC3Inst::Ldr(old, addr, Imm::Int(0)), Some("load for ++/--".to_string())));
                        self.regfile[old.value] = Self::USED;

                        let delta = if *op == UnaryOpType::Increment { 1 } else { -1 };
                        if *order {
                            // 前缀：新值写回并作为结果
                            self.emit_add_imm(old, delta);
                            emit!(self, Instruction(LC3Inst::Str(old, addr, Imm::Int(0)), Some("store back".to_string())));
                            self.regfile[addr.value] = Self::UNUSED;
                            return old;
                        }
                        else {
                            // 后缀：返回旧值，新值写回
                            let new = self.get_empty_reg();
                            emit!(self, Instruction(LC3Inst::AddImm(new, old, Imm::Int(delta)), None));
                            emit!(self, Instruction(LC3Inst::Str(new, addr, Imm::Int(0)), Some("store back".to_string())));
                            self.regfile[new.value] = Self::UNUSED;
                            self.regfile[addr.value] = Self::UNUSED;
                            return old;
                        }
                    }
                }
            },
            ASTNode::FunctionCall { symbol_ref, arguments } => {

                // TODO:
                // Push in-use regs

                let mut caller_saved: Vec<Register> = Vec::new();

                for n in 0..7 {
                    if self.regfile[n] == true {
                        let reg = Register {value: n};
                        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
                        emit!(self, Instruction(LC3Inst::Str(reg, Self::R6, Imm::Int(0)), Some(format!("caller save {reg}"))));
                        caller_saved.push(reg);
                        emit!(self, Newline);
                    }
                }


                // Push arguments right to left.
                for arg in arguments.into_iter().rev() {
                    let arg_reg = self.emit_expression_node(arg);
                    emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
                    emit!(self, Instruction(LC3Inst::Str(arg_reg, Self::R6, Imm::Int(0)), Some("push argument to stack.".to_string())));
                    emit!(self, Newline);
                    self.regfile[arg_reg.value] = Self::UNUSED;
                    // Emit newline
                }

                // Todo: Support calling arbitrary memory locations as functions.
                let entry = self.symbol_table.entries.get(*symbol_ref).unwrap();

                let identifier = self.context.resolve_string(entry.identifier);
                emit!(self, Newline);
                // Emit jump:
                emit!(self, Instruction(LC3Inst::Jsr(Label::Label(identifier.to_string())), Some("call function.".to_string())));
                emit!(self, Newline);
                // Handle return value.
                let ret = self.get_empty_reg();
                emit!(self, Instruction(LC3Inst::Ldr(ret, Self::R6, Imm::Int(0)), Some("load return value.".to_string())));
                emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(1)), None));

                let num_args: i32 = arguments.len().try_into().unwrap();
                if num_args != 0 {
                    emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(num_args)), Some("pop arguments".to_string())));
                }

                // Restore regs（逆序弹栈，与压栈顺序互逆）
                for reg in caller_saved.into_iter().rev() {
                    emit!(self, Newline);
                    emit!(self, Instruction(LC3Inst::Ldr(reg, Self::R6, Imm::Int(0)), Some(format!("caller restore {reg}"))));
                    emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(1)), None));
                    emit!(self, Newline);
                }
                emit!(self, Newline);
                return ret;
            },
            ASTNode::IntLiteral { value } => {
                let reg = self.get_empty_reg();
                self.emit_load_const(reg, *value, None);
                self.regfile[reg.value] = Self::USED;
                return reg;
            }
            ASTNode::SymbolRef { identifier } => {
                let entry = self.symbol_table.entries.get(*node_h).unwrap();
                let reg = self.get_empty_reg();
                let identifier = self.context.resolve_string(entry.identifier);

                if self.casts.get(*node_h) == Some(&TypeCast::LvalueToRvalue) {

                    let label = self.scalar_storage(entry);

                    if let Some(label) = label {
                        // 全局或静态局部：经标号 LD
                        emit!(self, Instruction(LC3Inst::Ld(reg, Label::Label(label)), Some(
                            format!("load variable '{identifier}'"))));
                    }
                    else {
                        if entry.kind == DeclarationType::Var {
                            emit!(self, Instruction(LC3Inst::Ldr(reg, Self::R5, Imm::Int(entry.offset)), Some(
                                format!("load local variable '{identifier}'"))));
                        }
                        else {
                            emit!(self, Instruction(LC3Inst::Ldr(reg, Self::R5, Imm::Int(entry.offset)), Some(
                                format!("load parameter '{identifier}'"))));
                        }
                    }
                }
                else if self.casts.get(*node_h) == Some(&TypeCast::ArrayToPointerDecay) {
                    // this is an lvalue, just generate the address
                    let label = self.scalar_storage(entry);
                    if let Some(label) = label {
                        emit!(self, Instruction(LC3Inst::Lea(reg, Label::Label(label)), Some(
                            format!("load base of array '{identifier}'"))));
                    }
                    else {
                        emit!(self, Instruction(LC3Inst::AddImm(reg, Self::R5, Imm::Int(entry.offset)), Some(
                            format!("load base of array access for '{identifier}'"))));
                    }
                }
                else {
                    // This is an Lvalue, just generate the Address
                    let label = self.scalar_storage(entry);
                    if let Some(label) = label {
                        emit!(self, Instruction(LC3Inst::Lea(reg, Label::Label(label)), Some(
                            format!("calculate address of '{identifier}'"))));
                    }
                    else {
                        emit!(self, Instruction(LC3Inst::AddImm(reg, Self::R5, Imm::Int(entry.offset)), Some(
                            format!("calculate address of '{identifier}'"))));
                    }


                }
                self.regfile[reg.value] = Self::USED;
                    return reg;

            }
            _ => {
                self.unsupported(node, "expression");
            }
        }
    }

    // __mul 运行时：重复加法实现乘法，遵循本编译器的调用约定
    // [R5+4] = a，[R5+5] = b，返回值写 [R5+3]； destroying R0-R3
    fn emit_mul_runtime(&mut self) -> () {
        emit!(self, Newline);
        emit!(self, SectionComment("__mul: a * b by repeated addition (supports negative operands)".to_string()));
        emit!(self, HeaderLabel(Label::Label("__mul".to_string()), None));

        // callee setup（与普通函数一致）
        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), Some("allocate spot for return value".to_string())));
        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
        emit!(self, Instruction(LC3Inst::Str(Self::R7, Self::R6, Imm::Int(0)), Some("push R7 (return address)".to_string())));
        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(-1)), None));
        emit!(self, Instruction(LC3Inst::Str(Self::R5, Self::R6, Imm::Int(0)), Some("push R5 (caller frame pointer)".to_string())));
        emit!(self, Instruction(LC3Inst::AddImm(Self::R5, Self::R6, Imm::Int(-1)), Some("set frame pointer".to_string())));

        let r0 = Register{value: 0};
        let r1 = Register{value: 1};
        let r2 = Register{value: 2};
        let r3 = Register{value: 3};

        emit!(self, Instruction(LC3Inst::Ldr(r0, Self::R5, Imm::Int(4)), Some("R0 = a".to_string())));
        emit!(self, Instruction(LC3Inst::Ldr(r1, Self::R5, Imm::Int(5)), Some("R1 = b".to_string())));
        emit!(self, Instruction(LC3Inst::AndImm(r2, r2, Imm::Int(0)), Some("R2 = 负操作数计数".to_string())));

        // 对 a 取绝对值，统计符号
        emit!(self, Instruction(LC3Inst::AddImm(r0, r0, Imm::Int(0)), None));
        emit!(self, Instruction(LC3Inst::Br(false, false, true, Label::Label("__mul.a_pos".to_string())), None));
        emit!(self, Instruction(LC3Inst::Not(r0, r0), None));
        emit!(self, Instruction(LC3Inst::AddImm(r0, r0, Imm::Int(1)), None));
        emit!(self, Instruction(LC3Inst::AddImm(r2, r2, Imm::Int(1)), None));
        emit!(self, HeaderLabel(Label::Label("__mul.a_pos".to_string()), None));

        // 对 b 取绝对值，统计符号
        emit!(self, Instruction(LC3Inst::AddImm(r1, r1, Imm::Int(0)), None));
        emit!(self, Instruction(LC3Inst::Br(false, false, true, Label::Label("__mul.b_pos".to_string())), None));
        emit!(self, Instruction(LC3Inst::Not(r1, r1), None));
        emit!(self, Instruction(LC3Inst::AddImm(r1, r1, Imm::Int(1)), None));
        emit!(self, Instruction(LC3Inst::AddImm(r2, r2, Imm::Int(1)), None));
        emit!(self, HeaderLabel(Label::Label("__mul.b_pos".to_string()), None));

        emit!(self, Instruction(LC3Inst::AndImm(r3, r3, Imm::Int(0)), Some("R3 = 结果".to_string())));

        emit!(self, HeaderLabel(Label::Label("__mul.loop".to_string()), None));
        emit!(self, Instruction(LC3Inst::AddImm(r1, r1, Imm::Int(0)), None));
        emit!(self, Instruction(LC3Inst::Br(false, true, false, Label::Label("__mul.done".to_string())), Some("b 用尽则结束".to_string())));
        emit!(self, Instruction(LC3Inst::AddReg(r3, r3, r0), None));
        emit!(self, Instruction(LC3Inst::AddImm(r1, r1, Imm::Int(-1)), None));
        emit!(self, Instruction(LC3Inst::Br(true, true, true, Label::Label("__mul.loop".to_string())), None));
        emit!(self, HeaderLabel(Label::Label("__mul.done".to_string()), None));

        // 奇数个负数 → 结果取负
        emit!(self, Instruction(LC3Inst::AddImm(r2, r2, Imm::Int(-1)), None));
        emit!(self, Instruction(LC3Inst::Br(true, false, true, Label::Label("__mul.ret".to_string())), Some("偶数个负数，结果为正".to_string())));
        emit!(self, Instruction(LC3Inst::Not(r3, r3), None));
        emit!(self, Instruction(LC3Inst::AddImm(r3, r3, Imm::Int(1)), None));
        emit!(self, HeaderLabel(Label::Label("__mul.ret".to_string()), None));

        emit!(self, Instruction(LC3Inst::Str(r3, Self::R5, Imm::Int(3)), Some("write return value, always R5 + 3".to_string())));

        // callee teardown（与普通函数一致）
        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R5, Imm::Int(1)), Some("pop local variables".to_string())));
        emit!(self, Instruction(LC3Inst::Ldr(Self::R5, Self::R6, Imm::Int(0)), Some("pop frame pointer".to_string())));
        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(1)), None));
        emit!(self, Instruction(LC3Inst::Ldr(Self::R7, Self::R6, Imm::Int(0)), Some("pop return address".to_string())));
        emit!(self, Instruction(LC3Inst::AddImm(Self::R6, Self::R6, Imm::Int(1)), None));
        emit!(self, Instruction(LC3Inst::Ret, None));
        emit!(self, SectionComment("end __mul.".to_string()));
    }
}
