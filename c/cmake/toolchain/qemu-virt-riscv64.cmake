set(CMAKE_ASM_COMPILER clang)
set(CMAKE_C_COMPILER clang)
set(CMAKE_CXX_COMPILER clang++)

set(COMMON_FLAGS -Wall -Wextra -Werror -fno-common -fno-builtin -nostdlib -ffreestanding)
set(COMMON_FLAGS ${COMMON_FLAGS} --target=${TARGET})
set(COMMON_FLAGS ${COMMON_FLAGS} -mabi=lp64d -march=rv64gc)
