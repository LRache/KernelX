use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs, thread};

const FILE_HEADER: &[u8; 7] = b"QPERF\0\x01";

struct Args {
    input: PathBuf,
    output: PathBuf,
    elf: PathBuf,
    addr2line: PathBuf,
    nm: PathBuf,
    address_map: Option<(u64, u64)>,
    unresolved: Option<PathBuf>,
    aggregate: bool,
}

struct Sample {
    vcpu_id: u32,
    addresses: Vec<u64>,
}

struct ElfSymbol {
    start: u64,
    end: u64,
    name: String,
}

struct Decoder {
    data: Vec<u8>,
    offset: usize,
}

impl Decoder {
    fn new(data: Vec<u8>) -> Result<Self, String> {
        if !data.starts_with(FILE_HEADER) {
            return Err("unsupported profiling file format; regenerate it with the matching qperf plugin".into());
        }
        Ok(Self {
            data,
            offset: FILE_HEADER.len(),
        })
    }

    fn is_eof(&self) -> bool {
        self.offset == self.data.len()
    }

    fn read_exact<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self
            .offset
            .checked_add(N)
            .ok_or_else(|| "input offset overflow".to_string())?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| format!("truncated qperf record at byte {}", self.offset))?;
        self.offset = end;
        Ok(bytes.try_into().unwrap())
    }

    fn read_varint(&mut self) -> Result<u64, String> {
        // The qperf plugin uses bincode 2's standard little-endian varint encoding.
        let marker = self.read_exact::<1>()?[0];
        match marker {
            0..=250 => Ok(marker.into()),
            251 => Ok(u16::from_le_bytes(self.read_exact()?).into()),
            252 => Ok(u32::from_le_bytes(self.read_exact()?).into()),
            253 => Ok(u64::from_le_bytes(self.read_exact()?)),
            _ => Err(format!(
                "invalid bincode varint marker {marker} at byte {}",
                self.offset - 1
            )),
        }
    }

    fn read_sample(&mut self) -> Result<Sample, String> {
        let vcpu_id =
            u32::try_from(self.read_varint()?).map_err(|_| format!("vCPU ID overflows u32 at byte {}", self.offset))?;
        let trace_len = usize::try_from(self.read_varint()?)
            .map_err(|_| format!("trace length overflows usize at byte {}", self.offset))?;
        if trace_len > self.data.len().saturating_sub(self.offset) {
            return Err(format!("invalid trace length {trace_len} at byte {}", self.offset));
        }

        let mut addresses = Vec::with_capacity(trace_len);
        for index in 0..trace_len {
            let ip = self.read_varint()?;
            if ip == 0 || ip == u64::MAX {
                continue;
            }
            addresses.push(if index == 0 { ip } else { ip - 1 });
        }
        Ok(Sample { vcpu_id, addresses })
    }
}

fn usage(program: &str) -> String {
    format!(
        "Usage: {program} --elf ELF [--addr2line PATH] [--nm PATH] \
         [--address-map PHYS:VIRT] [--unresolved PATH] [--aggregate] INPUT OUTPUT"
    )
}

fn parse_address(value: &str) -> Result<u64, String> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u64::from_str_radix(digits, 16).map_err(|error| format!("invalid address {value}: {error}"))
}

fn parse_address_map(value: &str) -> Result<(u64, u64), String> {
    let (physical, virtual_address) = value
        .split_once(':')
        .ok_or_else(|| format!("invalid address map {value}; expected PHYS:VIRT"))?;
    Ok((parse_address(physical)?, parse_address(virtual_address)?))
}

fn parse_args() -> Result<Args, String> {
    let mut argv = env::args();
    let program = argv.next().unwrap_or_else(|| "qperf-symbolize".into());
    let mut elf = None;
    let mut addr2line = PathBuf::from("addr2line");
    let mut nm = PathBuf::from("nm");
    let mut address_map = None;
    let mut unresolved = None;
    let mut aggregate = false;
    let mut positional = Vec::new();

    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--elf" | "-e" => {
                elf =
                    Some(PathBuf::from(argv.next().ok_or_else(|| {
                        format!("missing value for {arg}\n{}", usage(&program))
                    })?));
            }
            "--addr2line" => {
                addr2line = PathBuf::from(
                    argv.next()
                        .ok_or_else(|| format!("missing value for --addr2line\n{}", usage(&program)))?,
                );
            }
            "--nm" => {
                nm = PathBuf::from(
                    argv.next()
                        .ok_or_else(|| format!("missing value for --nm\n{}", usage(&program)))?,
                );
            }
            "--address-map" => {
                address_map =
                    Some(parse_address_map(&argv.next().ok_or_else(|| {
                        format!("missing value for --address-map\n{}", usage(&program))
                    })?)?);
            }
            "--unresolved" => {
                unresolved =
                    Some(PathBuf::from(argv.next().ok_or_else(|| {
                        format!("missing value for --unresolved\n{}", usage(&program))
                    })?));
            }
            "--aggregate" => aggregate = true,
            "--help" | "-h" => {
                println!("{}", usage(&program));
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown option: {arg}\n{}", usage(&program)));
            }
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    if positional.len() != 2 {
        return Err(usage(&program));
    }
    Ok(Args {
        input: positional.remove(0),
        output: positional.remove(0),
        elf: elf.ok_or_else(|| format!("--elf is required\n{}", usage(&program)))?,
        addr2line,
        nm,
        address_map,
        unresolved,
        aggregate,
    })
}

fn parse_qperf(path: &Path) -> Result<Vec<Sample>, String> {
    let data = fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut decoder = Decoder::new(data)?;
    let mut samples = Vec::new();
    while !decoder.is_eof() {
        samples.push(decoder.read_sample()?);
    }
    Ok(samples)
}

fn is_address_line(line: &str, expected: &HashSet<u64>) -> Option<u64> {
    let address = line.strip_prefix("0x")?;
    let address = u64::from_str_radix(address, 16).ok()?;
    expected.contains(&address).then_some(address)
}

fn sanitize_frame(frame: &str) -> String {
    frame.trim().replace([';', '\n', '\r'], ":")
}

fn is_local_label(frame: &str) -> bool {
    frame.starts_with(".L") || frame.starts_with('$')
}

fn load_symbols(tool: &Path, elf: &Path) -> Result<Vec<ElfSymbol>, String> {
    let output = Command::new(tool)
        .args(["-n", "-S", "--defined-only"])
        .arg(elf)
        .output()
        .map_err(|error| format!("failed to start {}: {error}", tool.display()))?;
    if !output.status.success() {
        return Err(format!("{} exited with status {}", tool.display(), output.status));
    }

    let text = String::from_utf8(output.stdout).map_err(|error| format!("nm produced non-UTF-8 output: {error}"))?;
    let mut symbols = Vec::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 4 || !matches!(fields[2], "T" | "t" | "W" | "w") {
            continue;
        }
        let Ok(start) = u64::from_str_radix(fields[0], 16) else {
            continue;
        };
        let Ok(size) = u64::from_str_radix(fields[1], 16) else {
            continue;
        };
        if size == 0 || is_local_label(fields[3]) {
            continue;
        }
        symbols.push(ElfSymbol {
            start,
            end: start.saturating_add(size),
            name: sanitize_frame(fields[3]),
        });
    }
    symbols.sort_unstable_by_key(|symbol| symbol.start);
    Ok(symbols)
}

fn enclosing_symbol(symbols: &[ElfSymbol], address: u64) -> Option<&str> {
    let index = symbols.partition_point(|symbol| symbol.start <= address);
    let symbol = symbols.get(index.checked_sub(1)?)?;
    (address < symbol.end).then_some(symbol.name.as_str())
}

fn symbolize(tool: &Path, nm: &Path, elf: &Path, addresses: &[u64]) -> Result<HashMap<u64, Vec<String>>, String> {
    if addresses.is_empty() {
        return Ok(HashMap::new());
    }

    let mut child = Command::new(tool)
        .args(["-a", "-f", "-C", "-i", "-e"])
        .arg(elf)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", tool.display()))?;
    let mut stdin = child.stdin.take().unwrap();
    let input_addresses = addresses.to_vec();
    let writer = thread::spawn(move || -> Result<(), String> {
        for address in input_addresses {
            writeln!(stdin, "0x{address:x}")
                .map_err(|error| format!("failed to send addresses to addr2line: {error}"))?;
        }
        Ok(())
    });
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for {}: {error}", tool.display()))?;
    writer
        .join()
        .map_err(|_| "addr2line input thread panicked".to_string())??;
    if !output.status.success() {
        return Err(format!("{} exited with status {}", tool.display(), output.status));
    }

    let expected: HashSet<u64> = addresses.iter().copied().collect();
    let text =
        String::from_utf8(output.stdout).map_err(|error| format!("addr2line produced non-UTF-8 output: {error}"))?;
    let mut result: HashMap<u64, Vec<String>> = HashMap::with_capacity(addresses.len());
    let mut current = None;
    let mut function_line = true;
    for line in text.lines() {
        if let Some(address) = is_address_line(line, &expected) {
            current = Some(address);
            function_line = true;
            result.entry(address).or_default();
            continue;
        }
        let Some(address) = current else {
            continue;
        };
        if function_line && line != "??" {
            result.get_mut(&address).unwrap().push(sanitize_frame(line));
        }
        function_line = !function_line;
    }
    let elf_symbols = load_symbols(nm, elf)?;
    for address in addresses {
        let frames = result.entry(*address).or_default();
        for frame in frames.iter_mut() {
            if is_local_label(frame)
                && let Some(symbol) = enclosing_symbol(&elf_symbols, *address)
            {
                *frame = symbol.into();
            }
        }
        frames.retain(|frame| !is_local_label(frame));
        frames.dedup();
        if frames.is_empty()
            && let Some(symbol) = enclosing_symbol(&elf_symbols, *address)
        {
            frames.push(symbol.into());
        }
    }
    Ok(result)
}

fn normalize_address(address: u64, address_map: Option<(u64, u64)>) -> u64 {
    let Some((physical_base, virtual_base)) = address_map else {
        return address;
    };
    // Early RISC-V frames can retain physical aliases after the kernel switches
    // to its high virtual mapping.
    if address < (1 << 63) && address >= physical_base {
        virtual_base.wrapping_add(address - physical_base)
    } else {
        address
    }
}

fn write_unresolved(
    path: &Path,
    unresolved_counts: &HashMap<u64, u64>,
    address_map: Option<(u64, u64)>,
) -> Result<(), String> {
    let mut rows: Vec<_> = unresolved_counts.iter().collect();
    rows.sort_unstable_by(|(left_address, left_count), (right_address, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_address.cmp(right_address))
    });

    let file = fs::File::create(path).map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    let mut output = BufWriter::new(file);
    writeln!(output, "address\tnormalized_address\tframe_occurrences")
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    for (address, count) in rows {
        let normalized = normalize_address(*address, address_map);
        writeln!(output, "0x{address:016x}\t0x{normalized:016x}\t{count}")
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    Ok(())
}

fn write_folded(
    args: &Args,
    samples: Vec<Sample>,
    symbols: &HashMap<u64, Vec<String>>,
) -> Result<(usize, u64), String> {
    let mut stacks: HashMap<Vec<String>, u64> = HashMap::new();
    let mut unresolved_counts: HashMap<u64, u64> = HashMap::new();

    for sample in samples {
        let mut frames = Vec::new();
        for address in sample.addresses {
            let normalized = normalize_address(address, args.address_map);
            match symbols.get(&normalized) {
                Some(symbols) if !symbols.is_empty() => frames.extend(symbols.iter().cloned()),
                _ => {
                    frames.push(format!("??@0x{address:016x}"));
                    *unresolved_counts.entry(address).or_default() += 1;
                }
            }
        }
        if frames.is_empty() {
            frames.push("??".into());
        }
        frames.reverse();
        if !args.aggregate {
            frames.insert(0, format!("[CPU {}]", sample.vcpu_id));
        }
        *stacks.entry(frames).or_default() += 1;
    }

    let unresolved_occurrences = unresolved_counts.values().sum();
    if let Some(path) = &args.unresolved {
        write_unresolved(path, &unresolved_counts, args.address_map)?;
    }

    let mut rows: Vec<_> = stacks.into_iter().collect();
    rows.sort_unstable_by(|(left_stack, left_count), (right_stack, right_count)| {
        right_count.cmp(left_count).then_with(|| left_stack.cmp(right_stack))
    });
    let unique_stacks = rows.len();

    let file = fs::File::create(&args.output)
        .map_err(|error| format!("failed to create {}: {error}", args.output.display()))?;
    let mut output = BufWriter::new(file);
    for (stack, count) in rows {
        writeln!(output, "{} {count}", stack.join(";"))
            .map_err(|error| format!("failed to write {}: {error}", args.output.display()))?;
    }
    Ok((unique_stacks, unresolved_occurrences))
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let samples = parse_qperf(&args.input)?;
    let total_samples = samples.len();

    let mut addresses: Vec<u64> = samples
        .iter()
        .flat_map(|sample| sample.addresses.iter().copied())
        .map(|address| normalize_address(address, args.address_map))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    addresses.sort_unstable();
    let symbols = symbolize(&args.addr2line, &args.nm, &args.elf, &addresses)?;
    let resolved_addresses = symbols.values().filter(|frames| !frames.is_empty()).count();
    let (unique_stacks, unresolved_occurrences) = write_folded(&args, samples, &symbols)?;

    eprintln!(
        "qperf-symbolize: {total_samples} samples, {} unique addresses ({} resolved), \
         {unique_stacks} unique stacks, {unresolved_occurrences} unresolved frame occurrences",
        addresses.len(),
        resolved_addresses
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("qperf-symbolize: {error}");
        std::process::exit(1);
    }
}
