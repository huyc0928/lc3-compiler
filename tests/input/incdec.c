int main() {
    int i = 0;
    int n = 0;
    while (i < 5) {
        n = n + i;
        i++;
    }
    int j = 3;
    int post = j++;
    int pre = ++j;
    return n + post * 10 + pre * 100;
}
