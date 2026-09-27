---
title: How to run a Backbeat Server
---

Running a server is easy.

## With the CLI

I recommend the CLI approach, as it also makes it easy to add content to your server.

You can install the cli with:

```sh
cargo install --locked bkb
```

If you have the [bkb CLI](https://crates.io/crates/bkb) installed, just run:

```sh
bkb start-data-server
```

This will run a fully working Backbeat server on port 8080. It will serve the contents of your store.

To add content, you can also use the CLI:

```sh
bkb download bundle $bundle_id
bkb download chart $chart_id

bkb collection add https://makiba.ac/tables/bms-7k/insane
bkb collection install-data https://makiba.ac/tables/bms-7k/insane

# etc...
```

Anything you add will be served as content. Note that serving collections is different from serving bundles and assets - to do that, read the [collections spec](backbeat-collection-endpoint)

## With Docker

If you don't want to install the backbeat CLI, you can always use docker:

```sh
docker run --rm -it \
  --publish 8080:8080 \
  --volume "$HOME/.config/backbeat:/root/.config/backbeat" \
  --volume "$HOME/.local/share/backbeat:/root/.local/share/backbeat" \
  ghcr.io/zkldi/backbeat-cli:latest \
  start-data-server
```

Understanding this is an exercise for the reader, or the readers LLM I guess.
