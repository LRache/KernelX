use crate::kernel::errno::Errno;
use super::*;

macro_rules! syscall_table {
    (
        $num_var:ident, $args_var:ident;
        $(
            $num:literal => $handler:ident :: $func:ident ( $arg_count:tt )
        ),* $(,)?
    ) => {
        match $num_var {
            $(
                $num => {
                    syscall_table!(@trace_enter $num, stringify!($func), $arg_count, $args_var);
                    let result = syscall_table!(@call $handler :: $func, $arg_count, $args_var);
                    syscall_table!(@trace_result $num, stringify!($func), $arg_count, $args_var, &result);
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

pub fn syscall(num: usize, args: &Args) -> Result<usize, Errno> {    
    syscall_table! {
        num, args;

        // Filesystem
        23  => fs::dup(1),
        24  => fs::dup2(2),
        25  => fs::fcntl64(3),
        29  => fs::ioctl(3),
        34  => fs::mkdirat(3),
        35  => fs::unlinkat(3),
        48  => fs::faccessat(3),
        56  => fs::openat(4),
        57  => fs::close(1),
        61  => fs::getdents64(3),
        62  => fs::lseek(3),
        63  => fs::read(3),
        64  => fs::write(3),
        65  => fs::readv(3),
        66  => fs::writev(3),
        71  => fs::sendfile(4),
        78  => fs::readlinkat(4),
        79  => fs::fstatat(4),
        80  => fs::newfstat(2),
        88  => fs::utimensat(4),
        276 => fs::renameat2(5),
        
        // Task
        17  => task::getcwd(2),
        49  => task::chdir(1),
        93  => task::exit(1),
        94  => task::exit_group(1),
        96  => task::set_tid_address(1),
        124 => task::sched_yield(0),
        172 => task::getpid(0),
        178 => task::gettid(0),
        220 => task::clone(5),
        221 => task::execve(3),
        260 => task::wait4(4),
        
        214 => mm::brk(1),
        215 => mm::munmap(2),
        222 => mm::mmap(6),
        226 => mm::mprotect(3),
        
        99  => misc::set_robust_list(0),
        160 => misc::newuname(1),
        261 => misc::prlimit64(4),
        293 => misc::rseq(0),

        174 => uid::getuid(0),
        175 => uid::geteuid(0),
        176 => uid::getgid(0),
        177 => uid::getegid(0),

        // IPC
        59  => ipc::pipe(2),
        129 => ipc::kill(2),
        134 => ipc::rt_sigaction(4),
        135 => ipc::rt_sigprocmask(3),
        137 => ipc::sigtimedwait(3),
        139 => ipc::rt_sig_return(0),

        // Time
        115 => time::clock_nanosleep(4),
        169 => time::gettimeofday(2),

        // Event
        73  => event::ppoll_time32(5),
    }
}
