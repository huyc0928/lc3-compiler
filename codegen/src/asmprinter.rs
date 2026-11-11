use core::fmt;
use std::{path::PathBuf, fs::File};
use std::io::Write;
use std::collections::HashMap;

/// 输出目标汇编器的兼容档位。
/// - Lc3Tools：默认，遵循本机 lc3tools 的宽松规则（标号可含 `_`/`.`、数字偏移等）。
/// - Suki：面向 suki-lc3 判题器（ICS OJ 后端）：标号仅限 1-20 位字母数字且字母开头、
///   大小写敏感、PC 相对操作数不接受数字偏移，标号统一在推入时改写以规避。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompatTarget {
    #[default]
    Lc3Tools,
    Suki,
}

pub struct AsmPrinter {
    instructions: Vec<LC3Bundle>,
    data: Vec<LC3Bundle>,
    compat: CompatTarget,
    // Suki 模式下原始标号 → 改写后标号，保证定义与引用一致
    label_map: HashMap<String, String>,
}

//TODO: Support 'builder' style api for these:

impl AsmPrinter {
    pub fn new() -> AsmPrinter {
        AsmPrinter { instructions: Vec::new(), data: Vec::new(), compat: CompatTarget::Lc3Tools, label_map: HashMap::new() }
    }

    pub fn set_compat(&mut self, compat: CompatTarget) -> () {
        self.compat = compat;
    }

    pub fn compat(&self) -> CompatTarget {
        self.compat
    }

    /// Suki 兼容模式是否开启（代码生成据此改用标号化的布尔序列）。
    pub fn suki_compat(&self) -> bool {
        self.compat == CompatTarget::Suki
    }

    pub fn inst(&mut self, inst: LC3Bundle) -> () {
        let inst = self.map_bundle(inst);
        self.instructions.push(inst);
    }

    pub fn data(&mut self, data: LC3Bundle) -> () {
        let data = self.map_bundle(data);
        self.data.push(data);
    }

    fn map_bundle(&mut self, bundle: LC3Bundle) -> LC3Bundle {
        if self.compat != CompatTarget::Suki {
            return bundle;
        }
        match bundle {
            LC3Bundle::HeaderLabel(label, comment) => {
                LC3Bundle::HeaderLabel(Label::Label(self.map_label(label.to_string())), comment)
            }
            LC3Bundle::Instruction(inst, comment) => {
                LC3Bundle::Instruction(self.map_inst_labels(inst), comment)
            }
            LC3Bundle::LabeledInstruction(label, inst, comment) => {
                let label = Label::Label(self.map_label(label.to_string()));
                LC3Bundle::LabeledInstruction(label, self.map_inst_labels(inst), comment)
            }
            LC3Bundle::Directive(label, directive, comment) => {
                let label = label.map(|l| Label::Label(self.map_label(l.to_string())));
                LC3Bundle::Directive(label, directive, comment)
            }
            other => other,
        }
    }

    fn map_inst_labels(&mut self, inst: LC3Inst) -> LC3Inst {
        match inst {
            LC3Inst::Br(n, z, p, label) => LC3Inst::Br(n, z, p, Label::Label(self.map_label(label.to_string()))),
            LC3Inst::Jsr(label) => LC3Inst::Jsr(Label::Label(self.map_label(label.to_string()))),
            LC3Inst::Ld(reg, label) => LC3Inst::Ld(reg, Label::Label(self.map_label(label.to_string()))),
            LC3Inst::Ldi(reg, label) => LC3Inst::Ldi(reg, Label::Label(self.map_label(label.to_string()))),
            LC3Inst::Lea(reg, label) => LC3Inst::Lea(reg, Label::Label(self.map_label(label.to_string()))),
            LC3Inst::St(reg, label) => LC3Inst::St(reg, Label::Label(self.map_label(label.to_string()))),
            LC3Inst::Sti(reg, label) => LC3Inst::Sti(reg, Label::Label(self.map_label(label.to_string()))),
            other => other,
        }
    }

    /// Suki 模式下的标号改写：去掉 `_` 与 `.`；运行时名（双下划线开头）加 X 前缀；
    /// 与助记符/寄存器冲突（大小写不敏感）时加 X；截断到 20 字符；冲突时续加 X。
    /// 同一原始名总是映射到同一结果。
    fn map_label(&mut self, name: String) -> String {
        if let Some(mapped) = self.label_map.get(&name) {
            return mapped.clone();
        }
        let mut base = name.replace(['_', '.'], "");
        if name.starts_with('_') {
            base.insert(0, 'X');
        }
        if reserved_word(&base.to_lowercase()).is_some() {
            base.push('X');
        }
        if base.len() > 20 {
            base.truncate(20);
        }
        let mut candidate = base.clone();
        while self.label_map.values().any(|v| v.eq_ignore_ascii_case(&candidate)) {
            if candidate.len() >= 20 {
                candidate.pop();
            }
            candidate.push('X');
        }
        self.label_map.insert(name, candidate.clone());
        candidate
    }

    // 输出 .asm。orig 形如 "x3000"/"0x3000"/"#12288"。
    // 输出前做 LC-3 合规校验：立即数范围、PC 相对偏移范围、标号合法性与唯一性。
    // 校验失败时返回所有问题，由调用方决定退出。
    pub fn print_to_file(&mut self, path: PathBuf, orig: &str) -> Result<(), String> {
        if self.compat == CompatTarget::Suki {
            self.suki_place_labels();
        }
        let errors = self.validate_and_layout(orig);

        let mut file = File::create(path).map_err(|e| format!("cannot open output file: {e}"))?;
        self.write_content(&mut file, orig)
            .map_err(|e| format!("write failed: {e}"))?;

        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        Ok(())
    }

    // Suki 兼容：标号安放预处理
    // 1) 连续的标号绑定同一地址，Suki 每行只允许一个标号：
    //    保留最后一个，其余作为别名，引用统一重定向。
    // 2) 标号不允许独占一行，把剩余标号并入下一条指令的同一行。
    fn suki_place_labels(&mut self) {
        // 1) 别名归并
        let mut alias: HashMap<String, String> = HashMap::new();
        let mut placed: Vec<LC3Bundle> = Vec::new();
        let mut last_kept: Option<String> = None;
        for bundle in self.instructions.drain(..) {
            match bundle {
                LC3Bundle::HeaderLabel(label, comment) => {
                    let name = label.to_string();
                    match &last_kept {
                        Some(kept) => {
                            alias.insert(name, kept.clone());
                            let _ = comment; // 被归并标号的注释一并丢弃
                        }
                        None => {
                            last_kept = Some(name.clone());
                            placed.push(LC3Bundle::HeaderLabel(label, comment));
                        }
                    }
                }
                LC3Bundle::Newline | LC3Bundle::SectionComment(_) => {
                    placed.push(bundle); // 不打断连续段
                }
                other => {
                    last_kept = None;
                    placed.push(other);
                }
            }
        }
        self.instructions = placed;

        // 2) 引用重定向
        if !alias.is_empty() {
            for bundle in &mut self.instructions {
                if let LC3Bundle::Instruction(inst, _) | LC3Bundle::LabeledInstruction(_, inst, _) = bundle {
                    rewrite_inst_labels(inst, &alias);
                }
            }
        }

        // 3) 标号并入下一条指令
        let mut merged: Vec<LC3Bundle> = Vec::new();
        let mut pending: Option<Label> = None;
        for bundle in self.instructions.drain(..) {
            match bundle {
                LC3Bundle::HeaderLabel(label, _) => { pending = Some(label); }
                LC3Bundle::Instruction(inst, comment) => {
                    if let Some(label) = pending.take() {
                        merged.push(LC3Bundle::LabeledInstruction(label, inst, comment));
                    } else {
                        merged.push(LC3Bundle::Instruction(inst, comment));
                    }
                }
                other => merged.push(other),
            }
        }
        self.instructions = merged;
    }

    fn write_content(&self, file: &mut File, orig: &str) -> std::io::Result<()> {
        // Temporary print prelude
        writeln!(file, "; ---------------------------------------------------------------------------")?;
        writeln!(file, "; C-LC3 Compiler, by HKN for UIUC Students                                   ")?;
        writeln!(file, "; Disclaimer: Not all C features are supported by this compiler. Do not base ")?;
        writeln!(file, "; assumptions about valid C programming on what you see here.                ")?;
        writeln!(file, "; Please report any bugs or unexpected crashes to <xrouth2@illinois.edu>     ")?;
        writeln!(file, "; To simulate output, use https://wchargin.com/lc3web/                       ")?;
        writeln!(file, "; ---------------------------------------------------------------------------")?;

        writeln!(file, ".ORIG {orig}")?;
        let mut prev_newline = false;
        for inst in &self.instructions {
            match inst {
                LC3Bundle::Newline => {
                    if prev_newline == true {
                        continue;
                    }
                    prev_newline = true;
                }
                _ => {
                    prev_newline = false;
                }
            };
            writeln!(file, "{inst}")?;
        }

        writeln!(file, "")?;
        writeln!(file, "; ------ Data Section ------ ")?;

        for datum in &self.data {
            writeln!(file, "{datum}")?;
        }

        let user_stack = self.fixed_label_name("USER_STACK");
        let return_slot = self.fixed_label_name("RETURN_SLOT");
        writeln!(file, "{user_stack:<17} .FILL xFDFF")?;
        writeln!(file, "{return_slot:<17} .FILL xFDFF")?;

        writeln!(file, ".END")?;

        Ok(())
    }

    // USER_STACK / RETURN_SLOT 的当前映射名（Suki 模式下推入时已登记）
    fn fixed_label_name(&self, original: &str) -> String {
        self.label_map
            .get(original)
            .cloned()
            .unwrap_or_else(|| original.to_string())
    }

    // 两遍处理：先分配地址并登记标号，再校验每条指令的字段范围。
    fn validate_and_layout(&self, orig: &str) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        let start_addr = match parse_addr(orig) {
            Some(a) => a,
            None => return vec![format!("invalid .ORIG address: {orig}")],
        };

        let mut labels: HashMap<String, u16> = HashMap::new();
        let mut addr: u32 = start_addr as u32;

        // 第一遍：地址布局 + 标号定义
        for bundle in self.instructions.iter().chain(self.data.iter()) {
            match bundle {
                LC3Bundle::HeaderLabel(label, _) => {
                    if self.compat == CompatTarget::Suki {
                        errors.push(format!("label '{}' 独占一行（Suki 要求标号与指令/伪指令同行）", label));
                    }
                    check_label(label, addr as u16, &mut labels, &mut errors, &mut warnings, self.compat);
                }
                LC3Bundle::LabeledInstruction(label, _, _) => {
                    check_label(label, addr as u16, &mut labels, &mut errors, &mut warnings, self.compat);
                    addr += 1;
                }
                LC3Bundle::Directive(Some(label), directive, _) => {
                    check_label(label, addr as u16, &mut labels, &mut errors, &mut warnings, self.compat);
                    addr += directive_words(directive) as u32;
                }
                LC3Bundle::Directive(None, directive, _) => {
                    addr += directive_words(directive) as u32;
                }
                LC3Bundle::Instruction(_, _) => {
                    addr += 1;
                }
                LC3Bundle::SectionComment(_) | LC3Bundle::Newline => {}
            }
        }

        // 输出末尾固定追加的两个数据字（见 write_content）
        let user_stack = Label::Label(self.fixed_label_name("USER_STACK"));
        let return_slot = Label::Label(self.fixed_label_name("RETURN_SLOT"));
        check_label(&user_stack, addr as u16, &mut labels, &mut errors, &mut warnings, self.compat);
        addr += 1;
        check_label(&return_slot, addr as u16, &mut labels, &mut errors, &mut warnings, self.compat);
        addr += 1;

        if addr > 0xFFFF {
            errors.push(format!("程序超出 LC-3 内存范围：结束地址 x{:04X} 越界", addr));
        }

        // 第二遍：指令字段范围
        let mut pc: u32 = start_addr as u32;
        for bundle in &self.instructions {
            match bundle {
                LC3Bundle::Instruction(inst, _) => {
                    validate_inst(inst, pc as u16, &labels, &mut errors, self.compat);
                    pc += 1;
                }
                LC3Bundle::LabeledInstruction(_, inst, _) => {
                    validate_inst(inst, pc as u16, &labels, &mut errors, self.compat);
                    pc += 1;
                }
                LC3Bundle::HeaderLabel(_, _) | LC3Bundle::Directive(_, _, _)
                | LC3Bundle::SectionComment(_) | LC3Bundle::Newline => {}
            }
        }

        errors
    }
}

// .ORIG 地址解析：x3000 / 0x3000 / #12288 / 12288
fn parse_addr(text: &str) -> Option<u16> {
    let t = text.trim().to_lowercase();
    let t = t.strip_prefix("#").unwrap_or(&t);
    let value = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("x")) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        t.parse::<u32>().ok()?
    };
    if value > 0xFFFF { return None; }
    Some(value as u16)
}

fn directive_words(d: &LC3Directive) -> usize {
    match d {
        LC3Directive::Fill(_) => 1,
        LC3Directive::Stringz(s) => s.chars().count() + 1,
        LC3Directive::Blkw(n) => (*n).max(0) as usize,
        LC3Directive::Orig | LC3Directive::End => 0,
    }
}

fn check_label(label: &Label, addr: u16, labels: &mut HashMap<String, u16>,
               errors: &mut Vec<String>, warnings: &mut Vec<String>, compat: CompatTarget) {
    let name = label.to_string();

    if compat == CompatTarget::Suki {
        // Suki：仅 1-20 位字母数字、字母开头（标号改写后仍需满足，作为安全网）
        if !is_valid_suki_label(&name) {
            errors.push(format!("invalid label '{name}': Suki 要求 1-20 位字母数字且字母开头"));
            return;
        }
    }
    else if !is_valid_label_syntax(&name) {
        errors.push(format!("invalid label '{name}': 只允许字母/数字/下划线/点，且不能以数字开头"));
        return;
    }
    let lower = name.to_lowercase();
    if let Some(reserved) = reserved_word(&lower) {
        errors.push(format!("invalid label '{name}': 与 LC-3 保留字 '{reserved}' 冲突（标号大小写不敏感）"));
        return;
    }
    if name.len() > 20 {
        // 教材建议但 lc3tools 不强制，仅警告
        warnings.push(format!("label '{name}' 超过 20 个字符（教材附录 A 建议）"));
    }
    if let Some(prev) = labels.insert(lower, addr) {
        errors.push(format!("duplicate label '{name}' (先前定义于 x{prev:04X})，lc3tools 标号大小写不敏感"));
    }
}

// Suki：1-20 位 ASCII 字母数字，字母开头
fn is_valid_suki_label(name: &str) -> bool {
    (1..=20).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric())
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

fn is_valid_label_syntax(name: &str) -> bool {
    if name.is_empty() { return false; }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') { return false; }
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

fn reserved_word(lower: &str) -> Option<&'static str> {
    const MNEMONICS: [&str; 29] = [
        "add", "and", "not", "br", "brn", "brz", "brp", "brnz", "brnp", "brzp", "brnzp",
        "jmp", "jsr", "jsrr", "ld", "ldi", "ldr", "lea", "st", "sti", "str",
        "rti", "trap", "ret", "getc", "out", "puts", "in", "putsp",
    ];
    if let Some(m) = MNEMONICS.iter().find(|m| **m == lower) {
        return Some(m);
    }
    const EXTRAS: [&str; 10] = [
        "halt", "nop", "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7",
    ];
    if let Some(m) = EXTRAS.iter().find(|m| **m == lower) {
        return Some(m);
    }
    None
}

fn rewrite_inst_labels(inst: &mut LC3Inst, alias: &HashMap<String, String>) {
    let fix = |name: String| -> String { alias.get(&name).cloned().unwrap_or(name) };
    match inst {
        LC3Inst::Br(_, _, _, label) => *label = Label::Label(fix(label.to_string())),
        LC3Inst::Jsr(label) => *label = Label::Label(fix(label.to_string())),
        LC3Inst::Ld(_, label) => *label = Label::Label(fix(label.to_string())),
        LC3Inst::Ldi(_, label) => *label = Label::Label(fix(label.to_string())),
        LC3Inst::Lea(_, label) => *label = Label::Label(fix(label.to_string())),
        LC3Inst::St(_, label) => *label = Label::Label(fix(label.to_string())),
        LC3Inst::Sti(_, label) => *label = Label::Label(fix(label.to_string())),
        _ => {}
    }
}

fn validate_inst(inst: &LC3Inst, pc: u16, labels: &HashMap<String, u16>, errors: &mut Vec<String>,
                 compat: CompatTarget) {
    match inst {
        LC3Inst::AddImm(_, _, imm) | LC3Inst::AndImm(_, _, imm) => {
            check_imm5(imm, pc, errors);
        }
        LC3Inst::Ldr(_, _, imm) | LC3Inst::Str(_, _, imm) => {
            check_offset6(imm, pc, errors);
        }
        LC3Inst::BrImm(_, _, _, imm) => {
            if compat == CompatTarget::Suki {
                // Suki 的 PC 相对操作数不接受数字偏移，代码生成应已全部标号化
                if let Immediate::Int(v) = imm {
                    errors.push(format!("x{pc:04X}: Suki 不接受数字 BR 偏移（{v}），应使用标号"));
                }
            }
            check_pcoffset9(imm, pc, errors, "BR");
        }
        LC3Inst::Br(_, _, _, label) => {
            check_label_offset(label, pc, labels, -256, 255, "BR", errors);
        }
        LC3Inst::Ld(_, label) | LC3Inst::Ldi(_, label) | LC3Inst::Lea(_, label)
        | LC3Inst::St(_, label) | LC3Inst::Sti(_, label) => {
            check_label_offset(label, pc, labels, -256, 255, "PCrelative", errors);
        }
        LC3Inst::Jsr(label) => {
            check_label_offset(label, pc, labels, -1024, 1023, "JSR", errors);
        }
        _ => {}
    }
}

fn check_imm5(imm: &Immediate, pc: u16, errors: &mut Vec<String>) {
    if let Immediate::Int(v) = imm {
        if !(-16..=15).contains(v) {
            errors.push(format!("x{pc:04X}: imm5 越界：{v} 不在 -16..15"));
        }
    }
}

fn check_offset6(imm: &Immediate, pc: u16, errors: &mut Vec<String>) {
    if let Immediate::Int(v) = imm {
        if !(-32..=31).contains(v) {
            errors.push(format!("x{pc:04X}: offset6 越界：{v} 不在 -32..31"));
        }
    }
}

fn check_pcoffset9(imm: &Immediate, pc: u16, errors: &mut Vec<String>, what: &str) {
    if let Immediate::Int(v) = imm {
        if !(-256..=255).contains(v) {
            errors.push(format!("x{pc:04X}: {what} 数字偏移越界：{v} 不在 -256..255"));
        }
    }
}

fn check_label_offset(label: &Label, pc: u16, labels: &HashMap<String, u16>,
                      lo: i32, hi: i32, what: &str, errors: &mut Vec<String>) {
    let name = label.to_string();
    match labels.get(&name.to_lowercase()) {
        None => errors.push(format!("x{pc:04X}: 引用了未定义标号 '{name}'")),
        Some(&target) => {
            let offset = target as i32 - (pc as i32 + 1);
            if offset < lo || offset > hi {
                errors.push(format!(
                    "x{pc:04X}: {what} 到标号 '{name}' (x{target:04X}) 的 PC 相对偏移 {offset} 超出 {lo}..{hi}"));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Label {
    Label(String)
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Label::Label(string) => write!(f, "{}", string)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Register {
    pub value: usize
}

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "R{}", self.value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Immediate {
    Hex(i32),
    Int(i32)
}

impl fmt::Display for Immediate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Immediate::Hex(val) => write!(f, "{:#06X}", val),
            Immediate::Int(val) => write!(f, "#{}", val),
        }
    }
}

pub enum LC3Inst {
    Noop,
    AddReg(Register, Register, Register),
    AddImm(Register, Register, Immediate),
    AndReg(Register, Register, Register),
    AndImm(Register, Register, Immediate),
    Br(bool, bool, bool, Label),
    BrImm(bool, bool, bool, Immediate),
    Jmp(Register),
    Jsr(Label),
    Jsrr(Register),
    Ld(Register, Label), // Should be Label or immediate, but almost always Label
    Ldi(Register, Label),
    Ldr(Register, Register, Immediate),
    Lea(Register, Label),
    Not(Register, Register),
    Ret,
    Rti, // What?
    St(Register, Label),
    Sti(Register, Label),
    Str(Register, Register, Immediate),
    Trap(String), // Trap vector.
    Halt // We should really sub-enum trap above ^^^
}

pub enum LC3Directive {
    Fill(i32),
    Stringz(String),
    Blkw(i32),
    Orig,
    End
}

impl fmt::Display for LC3Directive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // 16 位补码统一按 4 位十六进制输出，保证在 LC-3 字范围内
            LC3Directive::Fill(value) => write!(f, ".FILL x{:04X}", (*value as u16) as u16),
            LC3Directive::Stringz(value) => write!(f, ".STRINGZ \"{value}\""),
            LC3Directive::Blkw(value) => write!(f, ".BLKW #{value}"),
            LC3Directive::Orig => write!(f, ".ORIG"),
            LC3Directive::End => write!(f, ".END"),
        }
    }
}

pub enum LC3Bundle {
    HeaderLabel(Label, Option<String>), // String is comment
    Instruction(LC3Inst, Option<String>), // String is comment
    // 标号与指令同行（Suki 不接受标号独占一行，打印前由 compat 处理生成）
    LabeledInstruction(Label, LC3Inst, Option<String>),
    Directive(Option<Label>, LC3Directive, Option<String>), // String is comment
    SectionComment(String),
    Newline,
}

impl fmt::Display for LC3Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LC3Bundle::HeaderLabel(label, comment) => {
                match comment {
                    Some(comment) => {
                        let label = format!("{label}");
                        write!(f, "{:40}; {comment}", label)},
                    None => write!(f, "{label}"),
                }
            }
            LC3Bundle::Instruction(inst, comment) => {
                match comment { // Note indentation here:
                    Some(comment) => {
                        let inst = format!("    {inst}");
                        write!(f, "{:40}; {comment}", inst)
                    },
                    None => write!(f, "    {inst}"),
                }
            },
            LC3Bundle::LabeledInstruction(label, inst, comment) => {
                match comment {
                    Some(comment) => write!(f, "{label} {inst:<34}; {comment}"),
                    None => write!(f, "{label} {inst}"),
                }
            },
            LC3Bundle::Directive(label, directive, comment) => {
                let directive = format!("{directive}");
                match (label, comment) {
                    (None, None) => write!(f, "{directive}"),
                    (None, Some(comment)) => write!(f, "{directive} {:<50}{comment}", ";"),
                    (Some(label), None) => {
                        let label = format!("{label}");
                        write!(f, "{:17} {directive}", label) },
                    (Some(label), Some(comment)) => write!(f, "{label} {directive} {:<50};{comment}", ";"),
                }
            },
            LC3Bundle::SectionComment(comment) => write!(f, "; {comment}"),
            LC3Bundle::Newline => {write!(f, "")},
        }
    }
}

impl fmt::Display for LC3Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LC3Inst::Noop => write!(f, "\n"),
            LC3Inst::AddReg(dest, src1, src2) =>  write!(f, "ADD {dest}, {src1}, {src2}"),
            LC3Inst::AddImm(dest, src1, src2) => write!(f, "ADD {dest}, {src1}, {src2}"),
            LC3Inst::AndReg(dest, src1, src2) =>  write!(f, "AND {dest}, {src1}, {src2}"),
            LC3Inst::AndImm(dest, src1, src2) => write!(f, "AND {dest}, {src1}, {src2}"),
            LC3Inst::Br(n, z, p, label) => {
                match (n, z, p) {
                    (true, true, true) => write!(f,   "BR   {label}"), // Same as BR
                    (true, true, false) => write!(f,  "BRnz {label}"),
                    (true, false, true) => write!(f,  "BRnp {label}"),
                    (true, false, false) => write!(f, "BRn  {label}"),
                    (false, true, true) => write!(f,  "BRzp {label}"),
                    (false, true, false) => write!(f, "BRz  {label}"),
                    (false, false, true) => write!(f, "BRp  {label}"),
                    (false, false, false) => write!(f, "; bad BR opcode"),
                }
            }
            LC3Inst::BrImm(n, z, p, imm) => {
                match (n, z, p) {
                    (true, true, true) => write!(f,   "BR   {imm}"), // Same as BR
                    (true, true, false) => write!(f,  "BRnz {imm}"),
                    (true, false, true) => write!(f,  "BRnp {imm}"),
                    (true, false, false) => write!(f, "BRn  {imm}"),
                    (false, true, true) => write!(f,  "BRzp {imm}"),
                    (false, true, false) => write!(f, "BRz  {imm}"),
                    (false, false, true) => write!(f, "BRp  {imm}"),
                    (false, false, false) => write!(f, "; bad BR opcode"),
                }
            }
            LC3Inst::Jmp(reg) => write!(f, "JMP {reg}"), // This is same as ret actually
            LC3Inst::Jsr(label) => write!(f, "JSR {label}"),
            LC3Inst::Jsrr(_) => todo!(),
            LC3Inst::Ld(reg, label) => write!(f, "LD  {reg}, {label}"),
            LC3Inst::Ldi(reg, label) => write!(f, "LDI {reg}, {label}"),
            LC3Inst::Ldr(arg1, arg2, arg3) => write!(f, "LDR {arg1}, {arg2}, {arg3}"),
            LC3Inst::Lea(reg, label) => write!(f, "LEA {reg}, {label}"),
            LC3Inst::Not(dst, src) => write!(f, "NOT {dst}, {src}"),
            LC3Inst::Ret => write!(f, "RET"), // This is a special case of jump
            LC3Inst::Rti => todo!(),
            LC3Inst::St(reg, label) => write!(f, "ST  {reg}, {label}"),
            LC3Inst::Sti(reg, label) => write!(f, "STI {reg}, {label}"),
            LC3Inst::Str(arg1, arg2, arg3) => write!(f, "STR {arg1}, {arg2}, {arg3}"),
            LC3Inst::Trap(_) => todo!(),
            LC3Inst::Halt => write!(f, "HALT")
        }

    }
}
