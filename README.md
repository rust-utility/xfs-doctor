# xfs-doctor description

## Legal

Dual-licensed under `MIT` or the [UNLICENSE](http://unlicense.org/).

## Features

## Usage

Add dependency to Cargo.toml:

```toml
[dependencies]
xfs-doctor = "0.1"
```

## Development

RegEx for VSCode:

```txt
(\S+)\s+(\S+);\s*/\*\s*(.+)\s*\*/
/// $3\n    pub $2: $1,
```
