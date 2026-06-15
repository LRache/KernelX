#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <sys/wait.h>

#define SHM_SIZE (16 * 4096)
#define SHM_KEY  0x1234

/* Fill pattern: use offset-based values so every byte is uniquely checkable */
static void fill_pattern(unsigned char *buf, size_t size, unsigned char seed) {
    for (size_t i = 0; i < size; i++)
        buf[i] = (unsigned char)((i + seed) & 0xFF);
}

static int check_pattern(const unsigned char *buf, size_t size, unsigned char seed) {
    for (size_t i = 0; i < size; i++) {
        unsigned char expected = (unsigned char)((i + seed) & 0xFF);
        if (buf[i] != expected) {
            fprintf(stderr, "MISMATCH at byte %zu: expected 0x%02x got 0x%02x\n",
                    i, expected, buf[i]);
            return -1;
        }
    }
    return 0;
}

int main(void) {
    /* Create shared memory */
    int shmid = shmget(SHM_KEY, SHM_SIZE, IPC_CREAT | IPC_EXCL | 0666);
    if (shmid < 0) {
        perror("shmget");
        return 1;
    }

    /* Synchronization pipes: p2c = parent→child signal, c2p = child→parent signal */
    int p2c[2], c2p[2];
    if (pipe(p2c) < 0 || pipe(c2p) < 0) {
        perror("pipe");
        shmctl(shmid, IPC_RMID, NULL);
        return 1;
    }

    pid_t pid = fork();
    if (pid < 0) {
        perror("fork");
        shmctl(shmid, IPC_RMID, NULL);
        return 1;
    }

    if (pid == 0) {
        /* ---- Child (process B) ---- */
        close(p2c[1]);
        close(c2p[0]);

        unsigned char *shm = shmat(shmid, NULL, 0);
        if (shm == (void *)-1) { perror("child shmat"); exit(1); }

        char sync;

        /* Wait for A to finish writing round 1 */
        if (read(p2c[0], &sync, 1) != 1) { perror("child read p2c"); exit(1); }

        /* B checks A's pattern */
        if (check_pattern(shm, SHM_SIZE, 0xAA) < 0) {
            fprintf(stderr, "[B] round-1 check FAILED\n");
            exit(1);
        }
        printf("[B] round-1 check passed\n");

        /* B writes its own pattern */
        fill_pattern(shm, SHM_SIZE, 0x55);
        printf("[B] wrote round-2 pattern\n");

        /* Signal A that B is done */
        sync = 1;
        if (write(c2p[1], &sync, 1) != 1) { perror("child write c2p"); exit(1); }

        shmdt(shm);
        close(p2c[0]);
        close(c2p[1]);
        exit(0);
    }

    /* ---- Parent (process A) ---- */
    close(p2c[0]);
    close(c2p[1]);

    unsigned char *shm = shmat(shmid, NULL, 0);
    if (shm == (void *)-1) {
        perror("parent shmat");
        shmctl(shmid, IPC_RMID, NULL);
        return 1;
    }

    /* A writes round 1 */
    fill_pattern(shm, SHM_SIZE, 0xAA);
    printf("[A] wrote round-1 pattern\n");

    /* Signal B */
    char sync = 1;
    if (write(p2c[1], &sync, 1) != 1) { perror("parent write p2c"); goto fail; }

    /* Wait for B to finish */
    if (read(c2p[0], &sync, 1) != 1) { perror("parent read c2p"); goto fail; }

    /* A checks B's pattern */
    if (check_pattern(shm, SHM_SIZE, 0x55) < 0) {
        fprintf(stderr, "[A] round-2 check FAILED\n");
        goto fail;
    }
    printf("[A] round-2 check passed\n");

    shmdt(shm);
    close(p2c[1]);
    close(c2p[0]);

    int status;
    waitpid(pid, &status, 0);
    shmctl(shmid, IPC_RMID, NULL);

    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        fprintf(stderr, "child exited abnormally\n");
        return 1;
    }

    printf("shm test PASSED\n");
    return 0;

fail:
    shmdt(shm);
    shmctl(shmid, IPC_RMID, NULL);
    return 1;
}
