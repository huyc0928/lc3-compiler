---
name: lc3-compile
description: >
  Compile C programs to LC-3 assembly with the lc3-compiler in this repository,
  then assemble, run, and verify them with lc3tools. Use when writing or
  checking C code that targets LC-3 (labs, OJ-style fixed-address programs),
  when a task mentions lc3-compile, LC-3, .asm output, or the LC-3 matrix
  transpose, or when generated LC-3 assembly fails to assemble or behaves
  incorrectly.
---

# lc3-compile: C to LC-3 assembly

Compile a teaching subset of C to LC-3 assembly that assembles with lc3tools.
The compiler guarantees LC-3 compliance of its output: it validates immediate
and offset ranges, label syntax/uniqueness, and reserved-word collisions
before writing the file, and refuses to emit bad assembly.

## Paths

| What | Path |
| --- | --- |
| Compiler binary | `target/release/lc3-compile` (build with `cargo build --release`) |
| Full usage guide (EN / 中文) | `docs/USAGE.md` / `docs/USAGE.zh-CN.md` |
| lc3tools binaries | `~/projects/ics/lc3tools/build/bin/{assembler,simulator}` |
| Regression suite | `python3 tests/verify.py [name-filter]` |
| pty simulator driver | `tests/sim_run.py` (importable) |
| Example lab deliverable | `../lab/lab2/lab2.c` -> `../lab/lab2/lab2-c.asm`, verified by `../lab/lab2/verify.py` |

## Compile

```bash
target/release/lc3-compile -o out.asm program.c
# optional: --orig x3000 (start address, default x3000)
# optional: --compat suki  (ICS OJ / Suki judge backend; default lc3tools)
```

Non-zero exit = parse error, unsupported feature (`error: unsupported
feature: <what>`), or LC-3 constraint violation. Read the message; the compiler
never produces output that lc3tools would reject.

## Target assemblers (--compat)

- `lc3tools` (default): standard implementation, accepts `_`/`.` in labels and
  numeric PC-relative offsets (`BRz #2`).
- `suki`: the ICS OJ judge (`suki-lc3`). Its assembler is stricter: labels are
  1-20 alphanumeric chars only (no `_`/`.`), one label per line (no label-only
  lines), PC-relative operands must be labels, `.FILL` takes no labels. In
  this mode the compiler rewrites generated labels (`USER_STACK`->`USERSTACK`,
  `main.while.0.end`->`mainwhile0end`, `__mul`->`Xmul`), replaces numeric
  branch offsets with labeled ones, and merges labels onto the following
  instruction line. Output remains valid for both assemblers. Verify with
  `python3 tests/verify.py --compat suki` plus the Suki-side acceptance test
  `suki_compat_outputs_assemble` in `suki-lc3/tests/repro_oj.rs`.

## Assemble and run

```bash
BIN=~/projects/ics/lc3tools/build/bin
$BIN/assembler out.asm --print-level=3     # success is NOT exit code 0 alone;
                                           # check the .obj file exists
$BIN/simulator out.obj
```

Simulator facts that bite:

- `mem`/`set` parse addresses with `std::stoi` base 0: write `0x30FF`, not
  `x30FF`.
- Piping commands into the simulator is unreliable (keyboard device steals
  stdin). Use a pty: `from sim_run import load_and_run` in `tests/`.
- After `HALT`, R0/R1 are clobbered by the OS; the program's return value from
  `main` is stored at `0xFDFF` (`STI R0, RETURN_SLOT`). Read memory, not regs.
- To re-run a program in the same simulator session: `restart`, `load <obj>`,
  re-`set` memory, `set pc 0x3000`, `run <limit>`. `restart` wipes memory.
- Always run with an instruction limit (`run 50000`) to bound infinite loops.

## C subset the compiler accepts

- Types: `int`, `int *`, local fixed-size arrays `int a[N]`, structs with
  `int`/pointer/struct fields. One 16-bit word per value; no `char`, `float`,
  `typedef`, `sizeof`, `unsigned`.
- Literals: decimal and hex (`0x30FF`).
- Operators: `+ - *` (`*` calls built-in `__mul`, signed, loops `|b|` times),
  `< <= > >= == !=`, `&& || !` (NO short-circuit), `& | ~`, `=`, prefix and
  postfix `++/--`, array/pointer/struct access (`a[i]`, `*(p+k)`, `&x`,
  `s.f`, `p->f`). No `/ %`, no casts, no compound assignment, no `^`, no
  shifts, no ternary, no strings, no I/O, no preprocessor.
- Control flow: `if/else`, `while`, `for` (conditions: any value, nonzero =
  true, negatives handled correctly). No `do-while`, `switch`, `break`,
  `continue`, `?:`.
- Variables: locals, `static` locals, globals. Initializers must be integer
  literals or omitted. Declarations inside loops are safe (frames are
  allocated once per function).
- Functions: definitions with bodies only; recursion works. `main` must exist.

## Coding rules for LC-3 (violations of these cause wrong programs or errors)

1. `int` is 16 bits; arithmetic wraps at ±32768.
2. `&&`/`||` evaluate both sides. Null-guard with nested `if`, never with `&&`.
3. Multiplication is a subroutine call (loops `|b|` times). In hot loops prefer
   incremental pointers (advance a row base by `i`) over computing `r*i`.
4. Pointer arithmetic steps one element = one word (not 4 bytes).
5. Only 4 expression registers (R0-R3). Split very long expressions into
   statements or the compiler fails with a register-allocation error.
6. Fixed addresses (labs/OJ): `int *dim = 0x30FF; int *mat = 0x3100;` then
   dereference. Row-major indexing: `*(mat + r*i + c)` or pointer walking.
7. Never name identifiers after LC-3 words: `add`, `br`, `ld`, `st`, `ret`,
   `halt`, `out`, `puts`, `r0`-`r7`, or `__mul` (compiler runtime). Labels are
   case-insensitive; `Foo` and `foo` collide.
8. Global arrays are unsupported (emit one `.FILL`); use local arrays or
   pointers into fixed memory.
9. Output structure: one `.ORIG` segment starting at `x3000` (or `--orig`),
   code, then data section, final instruction `HALT` (TRAP x25), `.END`.

## Verification workflow (do this for any generated program)

1. `python3 tests/verify.py` — full suite must stay green (it compiles every
   `tests/input/*.c`, assembles, simulates via pty, and compares `main`'s
   return value at `0xFDFF` with gcc's semantics for the same source).
2. For a specific program, mirror `../lab/lab2/verify.py`: preload memory with
   `set 0xADDR 0xVAL`, `set pc 0x3000`, `run <limit>`, read back the memory
   window with `mem 0xSTART 0xEND`, compare against hand-computed expectations.
3. When behavior differs from C semantics, first re-read the generated `.asm`
   around the failing statement (compiler comments name the variable/operation)
   before suspecting the compiler; then reduce to a minimal probe under
   `target/probe/` (gitignored) and check it in as a `tests/input/*.c` case
   with a fix.

## Known-good example

`../lab/lab2/lab2.c` compiles to `../lab/lab2/lab2-c.asm` and passes in-place
matrix transpose for sizes 2/3/4/8 in the lc3tools simulator, matching the
hand-written reference `../lab/lab2/lab2.asm` exactly on the same inputs.
