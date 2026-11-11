# lc3-compiler Usage Guide

A C-to-LC-3 compiler for education, targeting the LC-3 ISA from *Introduction to
Computing Systems* (Patt & Patel). It compiles a subset of C to LC-3 assembly
that assembles with [lc3tools](https://github.com/chiragsakhuja/lc3tools).

中文版：[USAGE.zh-CN.md](USAGE.zh-CN.md)

## 1. Build

```bash
cd lc3-compiler
cargo build --release
# binary: target/release/lc3-compile
```

## 2. Command line

```
lc3-compile [OPTIONS] <INPUT_FILE>

Options:
  -o, --output <FILE>   output assembly path (default: ./out.asm)
      --orig <ADDR>     .ORIG start address (default: x3000; accepts x3000,
                        0x3000, #12288)
      --compat <TARGET> target assembler: lc3tools (default) or suki
                        (ICS OJ judge backend)
  -v, --verbose         print AST and symbol table
  -g, --debug           enable debug information
      --sandbox         disable most analysis errors (type checking)
  -S, --asm             (reserved)
  -h, --help            print help
```

Example:

```bash
target/release/lc3-compile -o out/out.asm program.c
```

The compiler exits non-zero on any failure: parse errors, unsupported
features, or LC-3 compliance violations (see section 6).

## 3. What the output looks like

Every program is emitted as one `.ORIG` segment:

```
.ORIG x3000
    LD  R6, USER_STACK     ; stack pointer, grows down from xFDFF
    ADD R5, R6, #-1        ; frame pointer for main
    JSR main
    HALT                   ; safety halt

main
    ...                    ; your code
    HALT                   ; end of main

other_function
    ...callee setup / body / teardown...
    RET

; ------ Data Section ------
GLOBAL_X     .FILL x0005
CONST_12544  .FILL x3100   ; constant pool (auto-generated)
USER_STACK   .FILL xFDFF
RETURN_SLOT  .FILL xFDFF
.END
```

- Globals and static locals become labels in the data section.
- Large immediate values (outside `imm5`: −16..15) are routed through an
  auto-generated constant pool (`CONST_*` labels) and loaded with `LD`.
- `main` writes its return value to `RETURN_SLOT` (`0xFDFF`) and halts. Run a
  program and read `0xFDFF` to observe the return value.
- The last executable instruction is always `HALT` (`TRAP x25`).

## 4. Runtime model (calling convention)

Follows the standard course convention:

| Resource | Use |
| --- | --- |
| R0–R3 | expression temporaries (all four allocatable; R4 unused) |
| R5 | frame pointer; locals at `R5+offset` (0, −1, −2, …) |
| R6 | stack pointer, grows down from `xFDFF` |
| R7 | link register (saved by callee) |
| Arguments | pushed right-to-left; first argument at `R5+4` of callee |
| Return value | stored at `R5+3` of callee; `main` stores to `0xFDFF` |
| `*` (multiply) | call to built-in `__mul` (repeated addition, signed) |

Recursion works; each call frame costs (args + 3 + locals) words of stack.

## 5. Supported C subset

**Types**: `int`, `int *`, one-dimensional arrays `int a[N]` (local, N an
integer literal), `struct` with `int`/pointer/struct fields. Every value is
one 16-bit word. There is no `char`, `float`, `long`, `unsigned`, `typedef`,
or `sizeof`.

**Literals**: decimal (`42`), hex (`0x30FF`). Negative constants are built
with the unary minus.

**Operators**:

| Category | Operators | Notes |
| --- | --- | --- |
| Arithmetic | `+` `-` `*` | `*` calls `__mul` (slow: loops `\|b\|` times) |
| Comparison | `<` `<=` `>` `>=` `==` `!=` | result normalized to 0/1 |
| Logical | `&&` `\|\|` `!` | **no short-circuit**: both sides always evaluated |
| Bitwise | `&` `\|` `~` | `\|` expands via De Morgan; no `^`, no shifts |
| Assignment | `=` | no compound assignment (`+=` etc.) |
| Access | `a[i]` `*(p+k)` `&x` `s.f` `p->f` | pointer arithmetic is per **element** (1 word) — this matches LC-3 word addressing |
| Inc/dec | `++` `--` | prefix and postfix |

**Control flow**: `if`/`else`, `while`, `for`. Any value can be a condition;
nonzero means true (normalized to 0/1 with correct sign handling). Not
supported: `do-while`, `switch`, `break`, `continue`, ternary `?:`, `goto`.

**Functions**: definitions only (no separate prototypes), recursion supported.
Function calls use the convention above; caller-saved temporaries are spilled
around calls. `main` must be defined; its `return` value is observable at
`0xFDFF`.

**Variables**: locals (auto), `static` locals, and globals. Initializers must
be integer literals (or absent → zero-filled). Local declarations may appear
in any block; space is allocated once per function in the prologue, so
declaring variables inside loops is safe.

**Fixed addresses** (lab/OJ style): assign a hex literal to a pointer, then
dereference:

```c
int *dim = 0x30FF;      /* matrix size lives at x30FF */
int *mat = 0x3100;      /* matrix data starts at x3100 */
int i = *dim;
*(mat + r * i + c) = v; /* A[r][c], row-major, 1 word per element */
```

## 6. LC-3 compliance enforcement

The compiler never writes assembly that lc3tools would reject. Before writing
the file it validates, in two passes over the final bundle list:

- `imm5` in −16..15, `offset6` (LDR/STR) in −32..31,
- `PCoffset9` (LD/LDI/ST/STI/LEA/BR) in −256..255, `offset11` (JSR) in
  −1024..1023, numeric `BR` offsets in −256..255,
- label syntax, duplicate labels (case-insensitive), collisions with
  mnemonics/registers/trap aliases (case-insensitive),
- total program size fits in memory from the `.ORIG` address.

Violations abort the compile with a message listing every problem. Function
names longer than 20 characters (plus generated suffixes like `.teardown`)
only produce a warning — lc3tools accepts them.

Reserved names you must not use as C identifiers: the mnemonics and aliases
(`add`, `br`, `ld`, `ret`, `halt`, `out`, `puts`, …), `r0`–`r7` (any case),
and the compiler runtime name `__mul`. Collisions are reported as
`invalid label 'add': ...`.

### `--compat suki` (ICS OJ / Suki judge backend)

The deployed OJ judges with `suki-lc3`, whose assembler is stricter than
lc3tools in ways that reject ordinary compiler output:

- labels allow only 1–20 ASCII **alphanumeric** characters (no `_`, no `.`);
- a label may not share its line spelling with any mnemonic/register
  (case-insensitive), and each line holds at most one label — a label on a
  line by itself is an error;
- PC-relative operands (`BR/LD/ST/LDI/STI/LEA/JSR`) must be labels, never
  numeric offsets (`BRz #2` is rejected);
- `.FILL` accepts only numeric values, not labels.

With `--compat suki` the compiler adapts automatically and the default
lc3tools output is untouched:

- generated labels are rewritten (`USER_STACK`→`USERSTACK`,
  `main.while.0.end`→`mainwhile0end`, `__mul`→`Xmul`, `CONST_12544`→
  `CONST12544`); C identifiers containing `_`/`.` are folded the same way,
  with `X` disambiguation when the fold collides;
- boolean normalization sequences use labeled branches instead of `#2`
  offsets;
- labels are placed on the same line as the following instruction; two labels
  that bind the same address (e.g. an `if`-exit label directly followed by a
  function's teardown label) are collapsed into one name via reference
  rewriting.

The result assembles with **both** assemblers — verify with
`python3 tests/verify.py --compat suki` (semantics via lc3tools simulation)
and the Suki-side acceptance test `suki_compat_outputs_assemble` in
`suki-lc3/tests/repro_oj.rs`.

## 7. C coding rules and pitfalls for LC-3

1. **`int` is 16 bits.** Values wrap at −32768..32767. Mental arithmetic from
   32-bit C does not transfer; keep intermediates small.
2. **`&&` and `||` do not short-circuit.** Both operands are always evaluated.
   Never rely on `p != 0 && *p == x` as a null guard — write nested `if`s.
3. **Multiplication is a subroutine.** `a * b` costs a call that loops `|b|`
   times. Avoid `*` inside hot loops; prefer incremental pointer updates
   (advance a row pointer by `i` instead of computing `r * i` per element).
4. **Pointer arithmetic advances one element (one word)**, not 4 bytes as on
   x86. `p + 1` is the next memory word — this is what you want on LC-3.
5. **Only 4 expression registers (R0–R3).** Very long expressions fail with
   `cannot allocate registers effectively`. Split them into separate
   statements: `t = a + b; c = t + d;`.
6. **PC-relative range is ±256 words.** A program with more than ~250
   instructions plus data fails the offset check. Split or simplify.
7. **Global arrays are not supported** (they emit a single `.FILL`). Use local
   arrays, or point at fixed memory with `int *p = 0x3100;`.
8. **No casts.** `int *p = 0x3100;` works (literal initializer), but
   `(int *)expr` is a parse error.
9. **No strings or I/O.** String literals, `printf`, `scanf`, `GETC`/`OUT`
   wrappers do not exist. There is no preprocessor: no `#define`, `#include`.
10. **Initializers must be literals.** `int x = y;` as a global/static
    initializer is ignored (treated as 0); initialize with assignment
    statements instead.
11. **Identifiers become case-insensitive labels.** Two globals `Foo` and
    `foo` collide; `int add;` collides with the `ADD` mnemonic.
12. **Dereferencing address 0** (e.g. an unset pointer) reads/writes the trap
    vector area — initialize pointers before use.

Unsupported constructs are rejected at compile time with
`error: unsupported feature: <what>` — they never produce silently wrong
assembly.

## 8. Assembling and running (lc3tools)

```bash
BIN=~/projects/ics/lc3tools/build/bin

$BIN/assembler out.asm                 # -> out.obj (also validates)
$BIN/simulator out.obj                 # interactive shell
#   run 50000        run with instruction limit (recommended)
#   mem 0xFDFF 0xFDFF  read the return value of main
#   set 0x30FF 3      preload memory (labs/OJ)
#   set pc 0x3000     reset PC before re-running
#   regs / step in / break set <addr> / quit
```

Notes from experience:

- The simulator's `mem`/`set` commands parse addresses with `std::stoi`
  base-0: write `0x30FF`, not `x30FF`.
- The assembler returns exit code 0 even on pass-2 errors — check that the
  `.obj` file exists, or use `--print-level=3`.
- Piping commands into the simulator is unreliable (the keyboard device
  competes for stdin). Use a pty — see `tests/sim_run.py`.

## 9. Test suite

```bash
python3 tests/verify.py                 # all inputs
python3 tests/verify.py hex             # name filter
```

For every `tests/input/*.c` it compiles, assembles, runs in the simulator
(pty-driven, `tests/sim_run.py`), and compares `main`'s return value at
`0xFDFF` against the semantics obtained by compiling the same file with the
system gcc. `tests/probe_check.py`-style one-off probes live in
`target/probe/` (not checked in).

## 10. Feature status

Implemented: hex literals, `+ - *`, comparisons, `&& || !`, `& | ~`,
`++/--`, pointers/arrays/structs, globals/static locals, recursion, lab-style
fixed addresses.

Not implemented (compile error, not bad code): `/ %`, casts, strings, I/O,
compound assignment, `^`, shifts, ternary, `switch`, `do-while`,
`break`/`continue`, function pointers, typedefs, global arrays, the C
preprocessor, unions.
