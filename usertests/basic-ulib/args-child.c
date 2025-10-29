#include <stdio.h>

int main(int argc, char **argv, char **envp) {
    puts("This args-child.");
    
    for (int i = 0; i < argc; i++) {
        puts(argv[i]);
    }

    for (int i = 0; envp[i] != NULL; i++) {
        puts(envp[i]);
    }

    return 0;
}
