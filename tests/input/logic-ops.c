int main() {
    int a = 1;
    int b = 2;
    int r = 0;
    if (a > 0 && b > 0) {
        r = r + 1;
    }
    if (a > 5 || b > 5) {
        r = r + 2;
    }
    if (!(a == 1)) {
        r = r + 4;
    }
    return r;
}
