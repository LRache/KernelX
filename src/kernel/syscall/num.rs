use crate::kernel::errno::Errno;

use super::*;

macro_rules! syscall_entries {
    ($callback:ident, $($prefix:tt)*) => {
        $callback! {
            $($prefix)*
            // Filesystem
            23  => fs::dup(1),
            24  => fs::dup3(3),
            25  => fs::fcntl64(3),
            29  => fs::ioctl(3),
            32  => fs::flock(2),
            34  => fs::mkdirat(3),
            39  => fs::umount2(2),
            40  => fs::mount(5),
            45  => fs::truncate64(2),
            35  => fs::unlinkat(3),
            36  => fs::symlinkat(3),
            37  => fs::linkat(4),
            43  => fs::statfs64(2),
            46  => fs::ftruncate64(2),
            47  => fs::fallocate(4),
            48  => fs::faccessat(3),
            52  => fs::fchmod(2),
            53  => fs::fchmodat(3),
            54  => fs::fchownat(5),
            55  => fs::fchown(3),
            56  => fs::openat(4),
            57  => fs::close(1),
            61  => fs::getdents64(3),
            62  => fs::lseek(3),
            63  => fs::read(3),
            64  => fs::write(3),
            65  => fs::readv(3),
            66  => fs::writev(3),
            67  => fs::pread64(4),
            68  => fs::pwrite64(4),
            69  => fs::preadv(4),
            70  => fs::pwritev(4),
            71  => fs::sendfile(4),
            76  => fs::splice(6),
            77  => fs::tee(4),
            78  => fs::readlinkat(4),
            79  => fs::fstatat(4),
            80  => fs::newfstat(2),
            82  => fs::fsync(1),
            88  => fs::utimensat(4),
            166 => fs::umask(1),
            276 => fs::renameat2(5),
            285 => fs::copy_file_range(6),
            286 => fs::preadv2(6),
            287 => fs::pwritev2(6),
            436 => fs::close_range(3),
            437 => fs::openat2(4),
            439 => fs::faccessat2(4),

            // Task
            17  => task::getcwd(2),
            49  => task::chdir(1),
            50  => task::fchdir(1),
            93  => task::exit(1),
            94  => task::exit_group(1),
            95  => task::waitid(5),
            96  => task::set_tid_address(1),
            124 => task::sched_yield(0),
            151 => task::setfsuid(1),
            152 => task::setfsgid(1),
            153 => misc::times(1),
            154 => task::setpgid(2),
            155 => task::getpgid(1),
            157 => task::setsid(0),
            158 => uid::getgroups(2),
            159 => uid::setgroups(2),
            172 => task::getpid(0),
            173 => task::getppid(0),
            178 => task::gettid(0),
            220 => task::clone(5),
            221 => task::execve(3),
            260 => task::wait4(4),
            272 => task::kcmp(5),
            281 => task::execveat(5),
            424 => ipc::pidfd_send_signal(4),
            434 => task::pidfd_open(2),
            435 => task::clone3(2),
            438 => task::pidfd_getfd(3),

            // Memory
            214 => mm::brk(1),
            215 => mm::munmap(2),
            222 => mm::mmap(6),
            226 => mm::mprotect(3),
            227 => mm::msync(3),
            228 => mm::mlock(2),
            233 => mm::madvise(0),

            // Futex
            98  => futex::futex(6),
            99  => futex::set_robust_list(1),
            100 => futex::get_robust_list(0),

            // Misc
            81  => misc::sync(0),
            92  => misc::personality(1),
            116 => misc::syslog(3),
            119 => misc::sched_setscheduler(3),
            120 => misc::sched_getscheduler(1),
            121 => misc::sched_getparam(2),
            122 => misc::sched_setaffinity(3),
            123 => misc::sched_getaffinity(3),
            140 => task::setpriority(3),
            141 => misc::getpriority(2),
            142 => misc::reboot(0),
            160 => misc::newuname(1),
            161 => misc::sethostname(2),
            162 => misc::setdomainname(2),
            165 => misc::getrusage(2),
            179 => misc::sysinfo(1),
            236 => misc::get_mempolicy(0),
            261 => misc::prlimit64(4),
            278 => misc::getrandom(3),
            283 => misc::membarrier(0),
            293 => misc::rseq(0),

            143 => uid::setregid(2),
            144 => uid::setgid(1),
            145 => uid::setreuid(2),
            146 => uid::setuid(1),
            147 => uid::setresuid(3),
            148 => uid::getresuid(3),
            149 => uid::setresgid(3),
            150 => uid::getresgid(3),
            174 => uid::getuid(0),
            175 => uid::geteuid(0),
            176 => uid::getgid(0),
            177 => uid::getegid(0),

            // IPC
            59  => ipc::pipe(2),
            129 => ipc::kill(2),
            130 => ipc::tkill(2),
            131 => ipc::tgkill(3),
            132 => ipc::sigaltstack(2),
            133 => ipc::rt_sigsuspend(1) [no_restart],
            134 => ipc::rt_sigaction(4),
            135 => ipc::rt_sigprocmask(3),
            136 => ipc::rt_sigpending(2),
            137 => ipc::sigtimedwait(4) [no_restart],
            138 => ipc::rt_sigqueueinfo(3),
            139 => ipc::rt_sig_return(0),
            194 => ipc::shmget(3),
            195 => ipc::shmctl(3),
            196 => ipc::shmat(3),
            197 => ipc::shmdt(1),

            // Network sockets
            198 => socket::socket(3),
            199 => socket::socketpair(4),
            200 => socket::bind(3),
            201 => socket::listen(2),
            202 => socket::accept(3),
            203 => socket::connect(3),
            204 => socket::getsockname(3),
            206 => socket::sendto(6),
            207 => socket::recvfrom(6),
            208 => socket::setsockopt(5),
            209 => socket::getsockopt(5),
            210 => socket::shutdown(2),
            211 => socket::sendmsg(3),
            212 => socket::recvmsg(3),

            // Time
            101 => time::nanosleep(2) [no_restart],
            107 => time::timer_create(3),
            108 => time::timer_gettime(2),
            109 => time::timer_getoverrun(1),
            110 => time::timer_settime(4),
            111 => time::timer_delete(1),
            113 => time::clock_gettime(2),
            114 => time::clock_getres(2),
            115 => time::clock_nanosleep(4) [no_restart],
            169 => time::gettimeofday(2),

            // Event
            19  => event::eventfd2(2),
            72  => event::pselect6_time32(6) [no_restart],
            73  => event::ppoll_time32(5) [no_restart],
            413 => event::pselect6_time64(6) [no_restart],
            85  => event::timerfd_create(2),
            86  => event::timerfd_settime(4),
            87  => event::timerfd_gettime(2),
            102 => event::getitimer(2),
            103 => event::setitimer(3),
        }
    };
}

macro_rules! dispatch_syscall_table {
    (
        $num_var:ident, $args_var:ident;
        $(
            $num:literal => $handler:ident :: $func:ident ( $arg_count:tt ) $( [ $policy:ident ] )?
        ),* $(,)?
    ) => {
        match $num_var {
            $(
                $num => {
                    dispatch_syscall_table!(@trace_enter $num, stringify!($func), $arg_count, $args_var);
                    let result = dispatch_syscall_table!(@call $handler :: $func, $arg_count, $args_var);
                    dispatch_syscall_table!(@trace_result $num, stringify!($func), $arg_count, $args_var, &result);
                    result
                },
            )*
            _ => {
                #[cfg(feature = "warn-unimplemented-syscall")]
                crate::kwarn!("Unsupported syscall: {}, user_pc={:#x}, tid={}", $num_var, crate::arch::get_user_pc(), crate::kernel::scheduler::current::tid());
                Err(Errno::ENOSYS)
            }
        }
    };

    (@trace_enter $num:expr, $name:expr, 0, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): ENTER args=[], tid={}", $num, $name, $crate::kernel::scheduler::current::tid());
        }
    };
    (@trace_enter $num:expr, $name:expr, 1, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): ENTER args=[{:#x}], tid={}", $num, $name, $args[0], $crate::kernel::scheduler::current::tid());
        }
    };
    (@trace_enter $num:expr, $name:expr, 2, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): ENTER args=[{:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $crate::kernel::scheduler::current::tid());
        }
    };
    (@trace_enter $num:expr, $name:expr, 3, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): ENTER args=[{:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], $crate::kernel::scheduler::current::tid());
        }
    };
    (@trace_enter $num:expr, $name:expr, 4, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): ENTER args=[{:#x}, {:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $crate::kernel::scheduler::current::tid());
        }
    };
    (@trace_enter $num:expr, $name:expr, 5, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): ENTER args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], $crate::kernel::scheduler::current::tid());
        }
    };
    (@trace_enter $num:expr, $name:expr, 6, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): ENTER args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], $args[5], $crate::kernel::scheduler::current::tid());
        }
    };

    (@trace_result $num:expr, $name:expr, 0, $args:ident, $result:expr) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            match $result {
                Ok(value) => println!("[SYSCALL] {} ({}): args=[] -> Ok({:#x}), tid={}", $num, $name, value, $crate::kernel::scheduler::current::tid()),
                // Ok(_) => {},
                Err(errno) => println!("[SYSCALL] {} ({}): args=[] -> Err({:?}), tid={}", $num, $name, errno, $crate::kernel::scheduler::current::tid()),
            }
        }
    };
    (@trace_result $num:expr, $name:expr, 1, $args:ident, $result:expr) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            match $result {
                Ok(value) => println!("[SYSCALL] {} ({}): args=[{:#x}] -> Ok({:#x}), tid={}", $num, $name, $args[0], value, $crate::kernel::scheduler::current::tid()),
                // Ok(_) => {},
                Err(errno) => println!("[SYSCALL] {} ({}): args=[{:#x}] -> Err({:?}), tid={}", $num, $name, $args[0], errno, $crate::kernel::scheduler::current::tid()),
            }
        }
    };
    (@trace_result $num:expr, $name:expr, 2, $args:ident, $result:expr) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            match $result {
                Ok(value) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}] -> Ok({:#x}), tid={}", $num, $name, $args[0], $args[1], value, $crate::kernel::scheduler::current::tid()),
                // Ok(_) => {},
                Err(errno) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}] -> Err({:?}), tid={}", $num, $name, $args[0], $args[1], errno, $crate::kernel::scheduler::current::tid()),
            }
        }
    };
    (@trace_result $num:expr, $name:expr, 3, $args:ident, $result:expr) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            match $result {
                Ok(value) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}] -> Ok({:#x}), tid={}", $num, $name, $args[0], $args[1], $args[2], value, $crate::kernel::scheduler::current::tid()),
                // Ok(_) => {},
                Err(errno) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}] -> Err({:?}), tid={}", $num, $name, $args[0], $args[1], $args[2], errno, $crate::kernel::scheduler::current::tid()),
            }
        }
    };
    (@trace_result $num:expr, $name:expr, 4, $args:ident, $result:expr) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            match $result {
                Ok(value) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}] -> Ok({:#x}), tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], value, $crate::kernel::scheduler::current::tid()),
                // Ok(_) => {},
                Err(errno) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}] -> Err({:?}), tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], errno, $crate::kernel::scheduler::current::tid()),
            }
        }
    };
    (@trace_result $num:expr, $name:expr, 5, $args:ident, $result:expr) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            match $result {
                Ok(value) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}] -> Ok({:#x}), tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], value, $crate::kernel::scheduler::current::tid()),
                // Ok(value) => {},
                Err(errno) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}] -> Err({:?}), tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], errno, $crate::kernel::scheduler::current::tid()),
            }
        }
    };
    (@trace_result $num:expr, $name:expr, 6, $args:ident, $result:expr) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            match $result {
                Ok(value) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}] -> Ok({:#x}), tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], $args[5], value, $crate::kernel::scheduler::current::tid()),
                Err(errno) => println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}] -> Err({:?}), tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], $args[5], errno, $crate::kernel::scheduler::current::tid()),
            }
        }
    };

    (@call $handler:ident :: $func:ident, 0, $args:ident) => {
        $handler::$func()
    };
    (@call $handler:ident :: $func:ident, 1, $args:ident) => {
        $handler::$func($args[0].into())
    };
    (@call $handler:ident :: $func:ident, 2, $args:ident) => {
        $handler::$func($args[0].into(), $args[1].into())
    };
    (@call $handler:ident :: $func:ident, 3, $args:ident) => {
        $handler::$func($args[0].into(), $args[1].into(), $args[2].into())
    };
    (@call $handler:ident :: $func:ident, 4, $args:ident) => {
        $handler::$func($args[0].into(), $args[1].into(), $args[2].into(), $args[3].into())
    };
    (@call $handler:ident :: $func:ident, 5, $args:ident) => {
        $handler::$func($args[0].into(), $args[1].into(), $args[2].into(), $args[3].into(), $args[4].into())
    };
    (@call $handler:ident :: $func:ident, 6, $args:ident) => {
        $handler::$func($args[0].into(), $args[1].into(), $args[2].into(), $args[3].into(), $args[4].into(), $args[5].into())
    };
}

macro_rules! syscall_restart_policy_table {
    (
        $num_var:ident;
        $(
            $num:literal => $handler:ident :: $func:ident ( $arg_count:tt ) $( [ $policy:ident ] )?
        ),* $(,)?
    ) => {
        match $num_var {
            $(
                $num => syscall_restart_policy_table!(@policy $( $policy )?),
            )*
            _ => false,
        }
    };

    (@policy) => {
        true
    };
    (@policy no_restart) => {
        false
    };
}

pub fn syscall(num: usize, args: &Args) -> Result<usize, Errno> {
    syscall_entries!(dispatch_syscall_table, num, args;)
}

pub fn should_restart_on_eintr(num: usize) -> bool {
    syscall_entries!(syscall_restart_policy_table, num;)
}
