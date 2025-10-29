#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

pid_t myfork() {
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        perror("fork failed");
        exit(1);
    }
    return pid;
}

int main() {
    /* ----- BASIC ------ */
    char *area1 = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (area1 == MAP_FAILED) {
        perror("mmap failed");
        return 1;
    }

    *(area1 + 20) = 'A';
    printf("area1[20]: %c\n", *(area1 + 20));
    /* ----- BASIC ------ */

    /* ----- FORK ----- */
    pid_t pid;
    int r, wstatus;

    if ((pid = myfork()) == 0) { // Child process
        printf("In child process\n");
        printf("area1[20] in child before change: %c\n", *(area1 + 20));
        *(area1 + 20) = 'B';
        printf("area1[20] in child after change: %c\n", *(area1 + 20));
        return 0;
    } 
        
    waitpid(pid, NULL, 0); // Wait for child to finish
    printf("In parent process\n");
    printf("area1[20] in parent: %c\n", *(area1 + 20));
    /* ----- FORK ----- */
    
    /* ----- MUNMAP ----- */
    if ((pid = myfork()) == 0) { // Child process
        r = munmap(area1, 4096);
        // if (r < 0) {
        //     perror("munmap failed");
        //     return 1;
        // }
        // *(area1 + 20) = 'C'; // This should trigger a segmentation fault
        // printf("area1[20] in child after unmap and change: %c\n", area1[20]);
        return 0;
    }
    waitpid(pid, &wstatus, 0); // Wait for child to finish
    printf("Parent: area1[20] after wait: %c\n", *(area1 + 20));
    /* ----- MUNMAP ----- */

    /* ----- MPROTECT ----- */
    printf("Parent: area1[20] before fork: %c\n", *(area1 + 20));
    if ((pid = myfork()) == 0) {
        printf("Children: area1[20] before mprotect: %c\n", area1[20]);
        
        r = mprotect(area1, 4096, PROT_READ);
        if (r < 0) {
            perror("mprotect failed");
            return 1;
        }

        *(area1 + 20) = 'D'; // This should trigger a segmentation fault
        printf("Children: area1[20] after mprotect and change: %c\n", area1[20]);
        return 0;
    }

    waitpid(pid, &wstatus, 0); // Wait for child to finish
    printf("Child exited with status: %d, area1[20]=%c\n", WEXITSTATUS(wstatus), area1[20]);
    /* ----- MPROTECT ----- */

    return 0;   
}
