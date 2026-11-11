#![deny(rust_2018_idioms)]
use std::{path::PathBuf, fs::File, io::Read, process::exit};
use analysis::{analysis::Analyzer, typecheck::{Typecheck, TypedASTPrint}};
use clap::{Parser};
//use codegen::asmprinter::AsmPrinter;
use lex_parse::{lexer, parser, ast::{ASTPrint, Vistior}, error::ErrorHandler, context::Context};

#[derive(Parser, Debug)]
#[command(name = "LC3-Compiler")]
#[command(about = "A C to LC3 Compiler built for students at the University of Illinois, \nUrbana-Champaign by HKN (https://hkn-alpha.netlify.app).\nReport bugs to <xrouth2@illinois.edu>.", long_about = None)]
#[command(author, version)] // Read from `Cargo.toml`
struct Cli {
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    // Produce verbose output (print AST)
    #[arg(short = 'v', long = "verbose", default_value_t = false)]
    verbose: bool,

    // Disables most analysis errors (type checking)
    #[arg(long = "sandbox", default_value_t = false)]
    sandbox: bool,

    // Output path
    #[arg(short = 'S', long = "asm", default_value_t = false)]
    asm: bool,

    // Enable debugging information
    #[arg(short = 'g', long = "debug", default_value_t = false)]
    debug: bool,

    // 程序起始地址（LC-3 汇编 .ORIG），默认 x3000
    #[arg(long = "orig", value_name = "ADDR", default_value = "x3000")]
    orig: String,

    // 目标汇编器兼容档位：lc3tools（默认，标准实现）或 suki（ICS OJ 判题器）
    #[arg(long = "compat", value_name = "TARGET", default_value = "lc3tools")]
    compat: String,

    #[arg(value_name = "INPUT_FILE")]
    input: PathBuf,

}

/// 解析 --compat；未知档位返回 Err 提示。
fn parse_compat(text: &str) -> Result<codegen::asmprinter::CompatTarget, String> {
    match text.to_lowercase().as_str() {
        "lc3tools" | "lc3-tools" | "default" => Ok(codegen::asmprinter::CompatTarget::Lc3Tools),
        "suki" | "oj" => Ok(codegen::asmprinter::CompatTarget::Suki),
        other => Err(format!(
            "unknown compat target '{other}' (expected 'lc3tools' or 'suki')")),
    }
}

fn main() {

    let cli = Cli::parse();

    let compat = match parse_compat(&cli.compat) {
        Ok(c) => c,
        Err(message) => {println!("error: {message}"); exit(1)},
    };

    let input_path: PathBuf  = cli.input;

    let mut input_file = match File::open(input_path.clone()) {
        Ok(input_file) => input_file,
        Err(_) => {println!("Invalid input file. {:?}", input_path); exit(1)},
    };

    let mut input_stream = String::new();

    input_file.read_to_string(&mut input_stream).unwrap();

    let context = Context::new(&input_stream);
    let error_handler: ErrorHandler<'_> = ErrorHandler::new(&context);

    let mut lexer: lexer::Lexer<'_, '_> = lexer::Lexer::new(&input_stream, &context, &error_handler);
    let parser: parser::Parser<'_> = parser::Parser::new(&mut lexer, &context, &error_handler);

    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(error) => {error_handler.print_parser_error(error); exit(1)},
    };

    let root =&ast.root.unwrap();
    if cli.verbose {
        let mut printer: ASTPrint<'_> = ASTPrint::new(false, &ast, &context);
        printer.traverse(root);
    }

    let mut analyzer: Analyzer<'_> = Analyzer::new(&ast, &context, &error_handler);

    // Check for analysis errors
    analyzer.traverse(root);

    if *error_handler.fatal.borrow() {
        exit(1);
    }

    if cli.verbose {
        analyzer.print_symbol_table();
    }

    let mut typecheck: Typecheck<'_> = Typecheck::new(&mut analyzer.symbol_table, &ast, &context, &error_handler);

    typecheck.traverse(root);

    if cli.verbose {
        let mut typed_printer = TypedASTPrint::new(false, &ast, &context, typecheck.types, typecheck.lr, typecheck.casts.clone());
        typed_printer.traverse(root);
    };

    let casts = typecheck.casts;

    let mut printer = codegen::asmprinter::AsmPrinter::new();
    printer.set_compat(compat);
    let mut codegen = codegen::codegen::Codegen::new(&ast, &mut printer, &context, analyzer.symbol_table, casts);

    codegen.emit_ast_node(&ast.root.unwrap());

    let outfile = match cli.output {
        Some(file) => file,
        None => PathBuf::from("out.asm")
    };
    if let Err(errors) = printer.print_to_file(outfile, &cli.orig) {
        println!("error: generated assembly violates LC-3 constraints:");
        println!("{}", errors);
        exit(1);
    }

}
