#include <stdio.h>

int main(int argc, char *argv[], char *envp[]) {
    printf("Hello from the child process!\n");
    for (int i = 0; i < argc; i++) {
        printf("Argument %d: %s\n", i, argv[i]);
    }
    for (int i = 0; envp[i] != NULL; i++) {
        printf("Environment variable %d: %s\n", i, envp[i]);
    }
    return 0;
}
