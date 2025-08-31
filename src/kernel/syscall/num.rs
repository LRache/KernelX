use crate::kernel::errno::Errno;
use crate::kernel::scheduler::current;
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
                    syscall_table!(@trace $num, stringify!($func), $arg_count, $args_var);
                    syscall_table!(@call $handler :: $func, $arg_count, $args_var)
                },
            )*
            _ => {
                crate::kwarn!("Unsupported syscall: {}, user_pc={:#x}, tid={}", $num_var, crate::arch::get_user_pc(), crate::kernel::scheduler::current::tid());
                Err(Errno::ENOSYS)
            }
        }
    };

    (@trace $num:expr, $name:expr, 0, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): args=[], tid={}", $num, $name, current::tid());
        }
    };
    (@trace $num:expr, $name:expr, 1, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): args=[{:#x}], tid={}", $num, $name, $args[0], current::tid());
        }
    };
    (@trace $num:expr, $name:expr, 2, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], current::tid());
        }
    };
    (@trace $num:expr, $name:expr, 3, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], current::tid());
        }
    };
    (@trace $num:expr, $name:expr, 4, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], current::tid());
        }
    };
    (@trace $num:expr, $name:expr, 5, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], current::tid());
        }
    };
    (@trace $num:expr, $name:expr, 6, $args:ident) => {
        #[cfg(feature = "log-trace-syscall")]
        {
            use crate::println;
            println!("[SYSCALL] {} ({}): args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}], tid={}", $num, $name, $args[0], $args[1], $args[2], $args[3], $args[4], $args[5], current::tid());
        }
    };
    
    (@call $handler:ident :: $func:ident, 0, $args:ident) => {
        $handler::$func()
    };
    (@call $handler:ident :: $func:ident, 1, $args:ident) => {
        $handler::$func($args[0])
    };
    (@call $handler:ident :: $func:ident, 2, $args:ident) => {
        $handler::$func($args[0], $args[1])
    };
    (@call $handler:ident :: $func:ident, 3, $args:ident) => {
        $handler::$func($args[0], $args[1], $args[2])
    };
    (@call $handler:ident :: $func:ident, 4, $args:ident) => {
        $handler::$func($args[0], $args[1], $args[2], $args[3])
    };
    (@call $handler:ident :: $func:ident, 5, $args:ident) => {
        $handler::$func($args[0], $args[1], $args[2], $args[3], $args[4])
    };
    (@call $handler:ident :: $func:ident, 6, $args:ident) => {
        $handler::$func($args[0], $args[1], $args[2], $args[3], $args[4], $args[5])
    };
}

pub fn syscall(num: usize, args: &Args) -> Result<usize, Errno> {    
    syscall_table! {
        num, args;

        // Filesystem
        23  => fs::dup(1),
        25  => fs::fcntl64(3),
        29  => fs::ioctl(3),
        48  => fs::faccessat(3),
        56  => fs::openat(4),
        57  => fs::close(1),
        63  => fs::read(3),
        64  => fs::write(3),
        66  => fs::writev(3),
        79  => fs::fstatat(4),
        80  => fs::newfstat(2),
        
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
        222 => mm::mmap(6),
        226 => mm::mprotect(3),
        
        135 => signal::rt_sigprocmask(3),

        // 99  => misc::set_robust_list(0),
        160 => misc::newuname(1),
        293 => misc::rseq(0),
    }
}
