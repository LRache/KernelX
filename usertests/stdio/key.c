#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>
#include <termios.h>

int main() {
    printf("sizeof(termios) = %2lx\n", sizeof(struct termios));
    // set raw mode
    struct termios tio;
    tcgetattr(STDIN_FILENO, &tio);
    tio.c_lflag &= ~(ICANON | ECHO); // disable canonical mode and
    tcsetattr(STDIN_FILENO, TCSANOW, &tio);
    
    char c;
    while (1) {
        int n = read(STDIN_FILENO, &c, 1);
        if (n != 0) {
            printf("%.2x\n", c);
        }
    }
    return 0;
}
