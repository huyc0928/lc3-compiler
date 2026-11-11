# lc3-compiler 使用指南

面向教学的 C→LC-3 编译器，目标指令集为《Introduction to Computing Systems》
（Patt & Patel）定义的 LC-3，产出的汇编可直接交给
[lc3tools](https://github.com/chiragsakhuja/lc3tools) 汇编。

English version: [USAGE.md](USAGE.md)

## 1. 构建

```bash
cd lc3-compiler
cargo build --release
# 可执行文件：target/release/lc3-compile
```

## 2. 命令行

```
lc3-compile [OPTIONS] <INPUT_FILE>

Options:
  -o, --output <FILE>   输出汇编路径（默认 ./out.asm）
      --orig <ADDR>     .ORIG 起始地址（默认 x3000；接受 x3000 / 0x3000 /
                        #12288 三种写法）
  -v, --verbose         打印 AST 与符号表
  -g, --debug           输出调试信息
      --sandbox         关闭大部分分析错误（类型检查）
  -S, --asm             （预留）
  -h, --help            帮助
```

示例：

```bash
target/release/lc3-compile -o out/out.asm program.c
```

任何失败（解析错误、不支持的特性、LC-3 合规违规）都以非零码退出，见第 6 节。

## 3. 输出结构

整个程序输出为单个 `.ORIG` 段：

```
.ORIG x3000
    LD  R6, USER_STACK     ; 栈指针，从 xFDFF 向下增长
    ADD R5, R6, #-1        ; main 的帧指针
    JSR main
    HALT                   ; 兜底停机

main
    ...                    ; 你的代码
    HALT                   ; main 结束

other_function
    ...被调方保存现场 / 函数体 / 恢复现场...
    RET

; ------ Data Section ------
GLOBAL_X     .FILL x0005
CONST_12544  .FILL x3100   ; 常量池（自动生成）
USER_STACK   .FILL xFDFF
RETURN_SLOT  .FILL xFDFF
.END
```

- 全局变量与 static 局部变量落在数据段，各占一个标号。
- 超出 `imm5`（−16..15）的立即数自动进入常量池（`CONST_*` 标号），用 `LD` 装入。
- `main` 的返回值经 `STI` 写到 `RETURN_SLOT`（`0xFDFF`）后停机；运行结束后读
  `0xFDFF` 即可观察返回值。
- 最后一条可执行指令始终是 `HALT`（即 `TRAP x25`）。

## 4. 运行时模型（调用约定）

遵循课程标准约定：

| 资源 | 用途 |
| --- | --- |
| R0–R3 | 表达式临时寄存器（共 4 个可用；R4 未用） |
| R5 | 帧指针；局部变量在 `R5+偏移`（0、−1、−2…） |
| R6 | 栈指针，从 `xFDFF` 向下增长 |
| R7 | 链接寄存器（被调方保存） |
| 参数 | 从右向左压栈；被调方的第一个参数在 `R5+4` |
| 返回值 | 写在 `R5+3`；`main` 写到 `0xFDFF` |
| `*`（乘法） | 调用内建运行时 `__mul`（重复加法，带符号） |

支持递归；每次调用占用（参数 + 3 + 局部变量）个字的栈空间。

## 5. 支持的 C 子集

**类型**：`int`、`int *`、一维数组 `int a[N]`（局部，N 为整数字面量）、
`struct`（字段可为 int/指针/结构体）。每个值占一个 16 位字。没有
`char`、`float`、`long`、`unsigned`、`typedef`、`sizeof`。

**字面量**：十进制（`42`）、十六进制（`0x30FF`）。负常数用一元负号构造。

**运算符**：

| 类别 | 运算符 | 说明 |
| --- | --- | --- |
| 算术 | `+` `-` `*` | `*` 调用 `__mul`（慢：循环 `\|b\|` 次） |
| 比较 | `<` `<=` `>` `>=` `==` `!=` | 结果归一化为 0/1 |
| 逻辑 | `&&` `\|\|` `!` | **无短路**：两边一定都会求值 |
| 位运算 | `&` `\|` `~` | `\|` 用德摩根律展开；没有 `^` 和移位 |
| 赋值 | `=` | 没有复合赋值（`+=` 等） |
| 访问 | `a[i]` `*(p+k)` `&x` `s.f` `p->f` | 指针按**元素**（1 个字）步进——与 LC-3 按字编址一致 |
| 自增自减 | `++` `--` | 前缀、后缀都支持 |

**控制流**：`if`/`else`、`while`、`for`。任何值都可以作条件，非零即真
（归一化为 0/1，负数语义正确）。不支持：`do-while`、`switch`、`break`、
`continue`、三目 `?:`、`goto`。

**函数**：仅支持带函数体的定义（没有单独的原型声明），支持递归。函数调用
遵循上述约定，活跃的临时寄存器在调用前后自动保存/恢复。`main` 必须定义；
其返回值可在 `0xFDFF` 处观察。

**变量**：局部变量（auto）、`static` 局部变量、全局变量。初始化必须是整数
字面量（缺省补 0）。局部声明可以出现在任意块中；空间在函数序言一次性分配，
所以**在循环体内声明变量是安全的**。

**固定地址**（实验/OJ 风格）：把十六进制字面量赋给指针后解引用：

```c
int *dim = 0x30FF;      /* 矩阵维数存放在 x30FF */
int *mat = 0x3100;      /* 矩阵数据从 x3100 开始 */
int i = *dim;
*(mat + r * i + c) = v; /* A[r][c]，行主序，每元素 1 个字 */
```

## 6. LC-3 合规校验

编译器不会输出 lc3tools 拒绝的汇编。写文件前对最终指令/数据序列做两遍校验：

- `imm5` 在 −16..15；`offset6`（LDR/STR）在 −32..31；
- `PCoffset9`（LD/LDI/ST/STI/LEA/BR）在 −256..255，`offset11`（JSR）在
  −1024..1023，`BR` 数字偏移在 −256..255；
- 标号语法、标号查重（大小写不敏感）、与助记符/寄存器/TRAP 别名冲突
  （大小写不敏感）；
- 从 `.ORIG` 起程序总长不超过内存。

违规即中止编译，并一次性列出所有问题。函数名（加上 `.teardown` 等生成后缀）
超过 20 个字符只产生警告——lc3tools 接受它们。

不能用作 C 标识符的保留名：助记符与别名（`add`、`br`、`ld`、`ret`、`halt`、
`out`、`puts`…）、`r0`–`r7`（任意大小写）、编译器运行时函数名 `__mul`。
冲突会报 `invalid label 'add': ...`。

## 7. 编写 C 时的注意事项（LC-3 特有）

1. **`int` 只有 16 位。** 数值在 −32768..32767 处回绕，32 位 C 的心算经验不
   适用，中间结果尽量小。
2. **`&&` 和 `||` 没有短路。** 两边都会求值，不要写 `p != 0 && *p == x` 当
   空指针保护——拆成嵌套 `if`。
3. **乘法是子程序调用。** `a * b` 会让 `__mul` 循环 `|b|` 次；热点循环里避免
   `*`，改用增量式指针更新（行基址每次 +i，而不是每个元素算 `r * i`）。
4. **指针算术按元素（1 个字）步进**，不是 x86 的 4 字节。`p + 1` 就是下一个
   内存字——LC-3 上这正是你想要的。
5. **表达式寄存器只有 4 个（R0–R3）。** 过长的表达式会报
   `cannot allocate registers effectively`，拆成多条语句：
   `t = a + b; c = t + d;`。
6. **PC 相对寻址范围 ±256 字。** 代码加数据超过约 250 条指令会触发偏移越界
   检查，需精简或分段。
7. **不支持全局数组**（只会输出一个 `.FILL`）。用局部数组，或用
   `int *p = 0x3100;` 指向固定内存。
8. **没有强制类型转换。** `int *p = 0x3100;` 可以（字面量初始化），但
   `(int *)expr` 是语法错误。
9. **没有字符串和 I/O。** 字符串字面量、`printf`、`scanf`、`GETC`/`OUT`
   封装都不存在；也没有预处理器：不能 `#define`、`#include`。
10. **初始化必须是字面量。** 全局/static 的 `int x = y;` 会被当成 0（忽略），
    应改用赋值语句初始化。
11. **标识符就是大小写不敏感的标号。** 两个全局 `Foo` 与 `foo` 冲突；
    `int add;` 与 `ADD` 助记符冲突。
12. **解引用地址 0**（未初始化的指针）会读写陷阱向量区——指针使用前先初始化。

不支持的构造在编译期以 `error: unsupported feature: <特性>` 明确拒绝，
绝不会生成"看起来对、跑起来错"的汇编。

## 8. 汇编与运行（lc3tools）

```bash
BIN=~/projects/ics/lc3tools/build/bin

$BIN/assembler out.asm                 # -> out.obj（同时做一次校验）
$BIN/simulator out.obj                 # 交互式 shell
#   run 50000        带指令数上限运行（推荐）
#   mem 0xFDFF 0xFDFF  读 main 返回值
#   set 0x30FF 3      预置内存（实验/OJ）
#   set pc 0x3000     重跑前复位 PC
#   regs / step in / break set <addr> / quit
```

实测经验：

- 模拟器的 `mem`/`set` 命令按 `std::stoi` base-0 解析地址：要写 `0x30FF`，
  不能写 `x30FF`。
- 汇编器 pass 2 失败时退出码仍是 0——判断成功要看 `.obj` 是否生成，或加
  `--print-level=3` 看报错。
- 直接用管道给模拟器喂命令不可靠（键盘设备会抢 stdin），要用伪终端，参考
  `tests/sim_run.py`。

## 9. 测试套件

```bash
python3 tests/verify.py                 # 全部用例
python3 tests/verify.py hex             # 按名字过滤
```

对每个 `tests/input/*.c`：编译 → lc3tools 汇编 → 模拟器运行（pty 驱动，见
`tests/sim_run.py`）→ 把 `main` 返回值（`0xFDFF`）与系统 gcc 运行同一程序的
语义对照。临时探针放 `target/probe/`（不入库）。

## 10. 特性状态

已实现：十六进制字面量、`+ - *`、比较、`&& || !`、`& | ~`、`++/--`、
指针/数组/结构体、全局与 static 局部变量、递归、实验式固定地址。

未实现（编译报错，不生成坏代码）：`/ %`、类型转换、字符串、I/O、复合赋值、
`^`、移位、三目、`switch`、`do-while`、`break`/`continue`、函数指针、
typedef、全局数组、C 预处理器、union。
