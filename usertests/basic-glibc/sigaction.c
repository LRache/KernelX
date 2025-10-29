#include <stdio.h>
#include <signal.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/wait.h>

pid_t Fork() {
    fflush(stdout);
    pid_t pid;
    if ((pid = fork()) < 0) {
        perror("fork error");
        exit(1);
    }
    return pid;
}

int Kill(pid_t pid, int sig) {
    if (kill(pid, sig) < 0) {
        perror("kill error");
        exit(1);
    }
    return 0;
}

void sigaction_quit() {
    printf("SIGACTION QUIT received!\n");
    exit(0);
}

int flag = 1;

void sigaction_usr1() {
    printf("SIGACTION USR1 received!\n");
    fflush(stdout);
    flag = 0;
}

int main() {
    pid_t pid;
    
    // pid = Fork();
    // if (pid == 0) {
    //     struct sigaction act;
    //     act.sa_handler = sigaction_quit;
    //     sigemptyset(&act.sa_mask);
    //     act.sa_flags = 0;
    //     if (sigaction(SIGQUIT, &act, NULL) < 0) {
    //         perror("sigaction error");
    //         exit(1);
    //     }
    //     while (1) {
    //         printf("Child process waiting for SIGQUIT...\n");
    //         sleep(5);
    //     }
    // } 

    // sleep(1);
    // printf("Parent process sending SIGQUIT to child process...\n");
    // Kill(pid, SIGQUIT);
    // wait(NULL);

    pid = Fork();

    if (pid == 0) {
        struct sigaction act;
        act.sa_handler = sigaction_usr1;
        sigemptyset(&act.sa_mask);
        sigaddset(&act.sa_mask, SIGUSR1);
        act.sa_flags = 0;
        if (sigaction(SIGUSR1, &act, NULL) < 0) {
            perror("sigaction error");
            exit(1);
        }
        while (flag) ;
        printf("Child process exiting after receiving SIGUSR1...\n");
        return 0;
    }

    sleep(1);
    Kill(pid, SIGUSR1);
    wait(NULL);
    
    return 0;
}
