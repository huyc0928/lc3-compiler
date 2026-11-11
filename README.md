# C-LC3-Compiler

C-LC3-Compiler is a modern, student built, C compiler targeting the LC3 Assembly language as described in *Introduction to Computing* by Dr. Yale Patt and Dr. Sanjay Patel. This tool is mainly for educational purposes, and is specifically meant to help students taking ECE 220 - Computer Systems & Programming at UIUC, however it should be relevant and useful to any student learning LC3 Assembly. The calling conventions implemented by the compiler mirror the conventions described in *Introduction to Computing*, any discrepancies are bugs.

This repository contains the source code for the compiler, as well as various tests. Currently, only a subset of the C language is supported. Some important features I am working on include:

- String literals
- Division and modulo subroutines
- I/O library functions
- Arrays (global arrays; local arrays already work)
- Structs and Unions
- Function Pointers
- C Preprocessor

### Supported C features (as of this fork)

- `int` / `int *` locals, parameters, globals and `static` locals; pointer dereference (`*p`), address-of (`&x`), array indexing (`a[i]`, `*(a+i)`), pointer arithmetic (1 word per element, matching LC-3 word addressing)
- Local arrays with constant size
- Structs (including nested field access `a.b` / `p->b`)
- Control flow: `if`/`else`, `while`, `for`; conditions normalize to 0/1 (nonzero is true)
- Arithmetic: `+` `-` `*` (via a built-in `__mul` runtime by repeated addition, negative operands supported); bitwise `&` `|` `~`; logical `&&` `||` `!` (no short-circuit); comparisons `<` `<=` `>` `>=` `==` `!=`
- Increment/decrement: `++`/`--` in prefix and postfix position
- Hex integer literals (`0x30FF`), decimal literals
- Recursion; the standard ECE-220 calling convention (args pushed right-to-left, return value at `R5+3`)

### LC-3 compliance enforcement

The emitted assembly is validated before writing: `imm5`/`offset6`/`PCoffset9`/`offset11` ranges, numeric `BR` offsets, label syntax/duplicates/collisions with reserved words (case-insensitive, matching lc3tools), and total program size. The program always starts at `.ORIG x3000` (override with `--orig`), ends with `HALT` (`TRAP x25`) and `.END`. Out-of-range immediates are automatically routed through a constant pool (`LD R0, CONST_xxx`).

Unsupported constructs are rejected with a clear `unsupported feature: ...` message instead of generating bad assembly.

### Verification

- `tests/verify.py` compiles every program in `tests/input/*.c`, assembles it with lc3tools, runs it in the lc3tools simulator (via a pty) and checks the value returned by `main` (stored at `0xFDFF` per the compiler's convention) against the semantics obtained from gcc.
- `tests/sim_run.py` is a reusable pty driver for the lc3tools simulator.


Some compiler explorer integration features I am working on include:

- Assembly documentation
- Instruction highlighting in source
- Code execution

### Build Instructions
To build the compiler locally,
1. Ensure you have Cargo installed. 
2. `git clone https://github.com/xavierrouth/lc3-compiler.git`
3. `cd lc3-compiler`
4. `cargo install --path driver`

To use the compiler without building from source I recommend using https://godbolt.org/.

### Usage Gudie
To run the compiler, use `lc3-compile`:

```
A C to LC3 Compiler built for students at the University of Illinois Urbana-Champaign by HKN (https://hkn-alpha.netlify.app).

Usage: lc3-compile [OPTIONS] <INPUT_FILE>

Arguments:
  <INPUT_FILE>  

Options:
  -o, --output <FILE>  
  -v, --verbose        
  -g, --debug          
  -h, --help           Print help
  -V, --version        Print version
```

To run the tests, run `python3 tests/verify.py` (requires a built lc3tools at `~/projects/ics/lc3tools` and the compiler at `target/release/lc3-compile`).

### LC3Tools
This project uses the [LC3Tools](https://github.com/chiragsakhuja/lc3tools) repository in order to assemble and verify the assembly generated from the compiler. Students working with LC3 may find tools from there helpful as well.

### Credits
The C-LC3-Compiler is developed and maintained by students from [HKN-Alpha](https://hkn-alpha.netlify.app/) at UIUC.

### Contributing and Bug Reporting
If you find a bug, please notify \<xrouth2 at illinois dot edu\>. We are always open to contributions, feel free to submit PRs or open discussion about what can be improved.
