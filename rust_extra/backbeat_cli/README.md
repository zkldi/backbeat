# `backbeat_cli (bkb)`

The backbeat CLI is a swiss army knife for interfacing with your backbeat store. You can think of it like a CLI interface to `backbeat_sdk`, plus some extras.

## Installation

You can install the backbeat CLI straight off of cargo:

```sh
cargo install --locked bkb --version 0.5.1
# Be aware that backbeat_cli is a crate owned by someone else
# and has nothing to do with us!
```

Or build it yourself in this repo:

```sh
cargo build --release
```

The CLI is also pushed to docker, in case you want to use it there:

```sh
docker run --rm -it ghcr.io/zkldi/backbeat-cli:latest
```

## Usage

I'm not going to document every command here, as commands are already documented inside the CLI, but here's the general usage:

```sh
# Show what you can do
bkb help

# Get information about your store
bkb info

# Start a backbeat server from your store
bkb start-data-server

# download a chart and all of its assets
bkb download chart 'md5/ff9f25ad005083c6173b565d054405c0'

# add a table
bkb collection add 'https://example.com/mytable'
# download everything needed for it
bkb collection install-data 'https://example.com/mytable'
```
