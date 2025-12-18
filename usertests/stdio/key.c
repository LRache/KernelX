#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main() {
    char c;
    while (1) {
        int n = read(STDIN_FILENO, &c, 1);
        if (n != 0) {
            printf("%.2x\n", c);
        }
    }
    return 0;
}
