# spv

spv: a minimalist, unix-inspired process supervisor for linux

## Features

- Does only one thing: supervise a child process
- Zero-config
- Handles child stdout and stderr

## Usage

The `<CMD>` argument is used as an identifier to manage your supervised processes. This means the same command you start with is used to reference it for all other operations.

### Running

```bash
spv start sleep 1000
```

Then you can manage it with the `<CMD>` argument:

```bash
spv logs sleep
spv restart sleep
spv stop sleep
```

### Running multiple instances

If you need to supervise more than one instance of the same command, use the `--id` flag to give each instance a unique identifier:

```bash
spv start --id sleep_1 sleep 1000
spv start --id sleep_2 sleep 1000
```

Now you can manage them individually:

```bash
spv stop sleep_1
spv logs sleep_2
```

## Installation

### Using Cargo

```bash
cargo install spv
```

Or build from source:

```bash
git clone https://github.com/juantascon/spv.git
cd spv
cargo install --path .
```

