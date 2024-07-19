# osmoinplace

Utility for setting up osmosis in-place testnet for development and testing.
The usual flow requires too much manual process and stitching together of commands, this utility aims to reduce the friction to get a local testnet with mainnet state up and running.

## Install

```
cargo install osmoinplace
```

## Usage

Make sure that `osmosisd` is in your `PATH`.

First, download your latest state snapshot and create a backup in case you need to restore it in you testing.

```sh
osmoinplace download-mainnet-state
osmoinplace backup
```

This, by default, should download the latest state snapshot and save it to `~/.osmosisd` and backup to `~/.osmosisd_bak`.

Now you can keep running clean in-place testnet from backup state by running:

```sh
osmoinplace magic-start
```

note that you can pipe `yes` to skip the interactive prompt like this

```sh
yes | osmoinplace magic-start
```

In case there is a version upgrade, make sure that you have the current and next version of `osmosisd` binary.
For this example, it has `osmosisd_v25` and `osmosisd_v26` in the `PATH`.

```sh
yes | osmoinplace --osmosisd-bin osmosisd_v25 magic-start  --upgrade-handler v26 --new-osmosisd-bin osmosisd_v26
```

This might take a while to complete, so if you want to run some command when the chain is ready, or just want to notify yourself, you can pass any shell script to `--on-ready` flag.

```sh
yes | osmoinplace --osmosisd-bin osmosisd_v25 magic-start  --upgrade-handler v26 --new-osmosisd-bin osmosisd_v26 --on-ready "say 'ready to rumble!'"
```
