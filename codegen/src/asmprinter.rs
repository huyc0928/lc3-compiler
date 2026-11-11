use core::fmt;
use std::{path::PathBuf, fs::File};
use std::io::Write;
use std::collections::HashMap;

pub struct AsmPrinter {
    instructions: Vec<LC3Bundle>,
    data: Vec<LC3Bundle>,
}

//TODO: Support 'builder' style api for these:

impl AsmPrinter {
    pub fn new() -> AsmPrinter {
        AsmPrinter { instructions: Vec::new(), data: Vec::new() }
    }
    pub fn inst(&mut self, inst: LC3Bundle) -> () {
        self.instructions.push(inst);
    }

    pub fn data(&mut self, data: LC3Bundle) -> () {
        self.data.push(data);
    }

    // 输出 .asm。orig 形如 "x3000"/"0x3000"/"#12288"。
    // 输出前做 LC-3 合规校验：立即数范围、PC 相对偏移范围、标号合法性与唯一性。
    // 校验失败时返回所有问题，由调用方决定退出。
    pub fn print_to_file(&mut self, path: PathBuf, orig: &str) -> Result<(), String> {
        let errors = self.validate_and_layout(orig);

        let mut file = File::create(path).map_err(|e| format!("cannot open output file: {e}"))?;
        self.write_content(&mut file, orig)
            .map_err(|e| format!("write failed: {e}"))?;

        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        Ok(())
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

        writeln!(file, "USER_STACK        .FILL xFDFF")?;
        writeln!(file, "RETURN_SLOT       .FILL xFDFF")?;

        writeln!(file, ".END")?;

        Ok(())
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
                    check_label(label, addr as u16, &mut labels, &mut errors, &mut warnings);
                }
                LC3Bundle::Directive(Some(label), directive, _) => {
                    check_label(label, addr as u16, &mut labels, &mut errors, &mut warnings);
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
        check_label(&Label::Label("USER_STACK".to_string()), addr as u16, &mut labels, &mut errors, &mut warnings);
        addr += 1;
        check_label(&Label::Label("RETURN_SLOT".to_string()), addr as u16, &mut labels, &mut errors, &mut warnings);
        addr += 1;

        if addr > 0xFFFF {
            errors.push(format!("程序超出 LC-3 内存范围：结束地址 x{:04X} 越界", addr));
        }

        // 第二遍：指令字段范围
        let mut pc: u32 = start_addr as u32;
        for bundle in &self.instructions {
            match bundle {
                LC3Bundle::Instruction(inst, _) => {
                    validate_inst(inst, pc as u16, &labels, &mut errors);
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
               errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    let name = label.to_string();

    if !is_valid_label_syntax(&name) {
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

fn validate_inst(inst: &LC3Inst, pc: u16, labels: &HashMap<String, u16>, errors: &mut Vec<String>) {
    match inst {
        LC3Inst::AddImm(_, _, imm) | LC3Inst::AndImm(_, _, imm) => {
            check_imm5(imm, pc, errors);
        }
        LC3Inst::Ldr(_, _, imm) | LC3Inst::Str(_, _, imm) => {
            check_offset6(imm, pc, errors);
        }
        LC3Inst::BrImm(_, _, _, imm) => {
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
