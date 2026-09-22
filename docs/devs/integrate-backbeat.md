---
title: I'm a developer, how do I integrate it into my game!
---

Backbeat is designed to disappear into your game. People install the Backbeat
application separately to get whatever they want,
and your game reads the content they've installed using the Backbeat sdk.

You do not need to run a Backbeat service alongside your game, it's just a library.

**Backbeat is GPL-3.0 software and is intended for open-source rhythm games. Check
that the license works for your game before doing anything.**

## 1. Add the SDK

### If your game is written in Rust

Backbeat is written in Rust, and `backbeat_sdk` (`cargo add backbeat_sdk`) is
the easiest way to get started in Rust.

### If your game is written in Java

[We provide a Java SDK](https://github.com/zkldi/backbeat/tree/main/java). It's a wrapper around the C SDK.

### Otherwise, use the C SDK

We provide first-class support for backbeat with our C SDK.
The SDK is also a stable C ABI, so it can be wrapped from another language, like the Java SDK above.

**Documentation for the C SDK is included in the source code and the shipped headers.**

Download the C SDK package from the [Backbeat releases page](https://github.com/zkldi/backbeat/releases).

The package contains the `backbeat.h` header, a static Backbeat library, a compatible SQLite
library, and CMake/pkg-config metadata.

With CMake, link the supplied targets:

```cmake
find_package(Backbeat CONFIG REQUIRED)

target_link_libraries(your_game PRIVATE Backbeat::Backbeat)
```

## 2. Things you probably want to implement

You will want to have a couple of concepts in your game first.

Here's the high level plan:

- You need a type that is either (path on disk, in memory bytes)
- You need a way of keeping track of `bundle_id`s for charts that are loaded from Backbeat
- You need a way of, on your charts, doing `chart.what_resource_is("song.ogg")`.

This way of integrating into Backbeat is the simplest in my opinion, but you can really do whatever.

### A Resource Type

You will need some structure that looks like this:

```rs
pub enum Resource {
	InMemoryBytes(Vec<u8>),
	Path(String)
}
```

Which is to say - something that you can pass around that is either a pointer to a file on disk, or is
just in-memory bytes. Important Backbeat APIs for resolving files return either a pointer to a file or the bytes themselves, so you will need to recognise this and handle it accordingly.

I strongly recommend against copying data/files or materialising them to other places on your disk, as that ends up being very inefficient.

### A backbeat_bundle_id field

In my opinion, the easiest way to implement Backbeat is to find your `Chart` struct, whatever you call it - your unit of gameplay, and add on a new column + field for it.

```rs
struct Chart {
	artist: String,
	title: String,
	// ...

	// Add this new column + field to your chart type.
	backbeat_bundle_id: Option<String>,
}
```

If your game uses sqlite to store data - and it really should - add this to your chart table too.

### A `resolve_asset` function on your chart

Finally, you'll want a function like this like this:

```rs
// pseudocode!!!
fn resolve_asset(chart: &Chart, path: &str) -> Option<Resource> {
	// I recommend doing something like this
	if chart.backbeat_bundle_id {
		return store.resolve_path(chart.backbeat_bundle_id, path)
	} else {
		return Resource::Path(chart.folder.join(path))
	}
}
```

With these things, the rest of integrating Backbeat should be fairly low effort, and require few changes
throughout your codebase.
**The most important thing you want to avoid, is making the entirety of your codebase suddenly have to care about whether a chart is from Backbeat or not. A good abstraction here will prevent tons of pain elsewhere.**

## 3. Opening the store

Open the store once during game startup and keep it alive while you are using
Backbeat:

=== "Rust"

    ```rust
    let store = backbeat_sdk::Backbeat::open()?;
    // and it closes itself when it goes out of scope, so put it somewhere.
    ```

=== "C"

    ```c
    bkb_store *store = NULL;
    bkb_error_code error = bkb_store_open(&store);
    if (error != BKB_OK) {
        /* Show the game a useful error, or run without Backbeat content. */
    }
    ```

=== "Java"

    ```java
    var store = Store.open();

    // then call Store.close() to kill it.
    ```

The SDK finds the user's normal Backbeat configuration and store location for you. There's no "store location" parameter that you - as a game - need to know or care about. That's the users choice, and it's extrinsic to your game.

Free the store when the game exits:

=== "Rust"

    ```rust
    drop(store);
    ```

=== "C"

    ```c
    bkb_store_free(store);
    ```

=== "Java"

    ```java
    store.close();
    ```

## 4. Getting the charts you care about

You can search installed bundles with `bkb_store_search_bundles`, passing a query,
offset, limit, and the extensions your game understands.

For example:

=== "Rust"

    ```rust
    let mut offset = 0;
    loop {
        let page = store.search_bundles(None, offset, 100, &["bms", "bme", "bml", "bmson"])?;
        let page_len = page.charts.len() as u64;
        let has_more = page.has_more;

        for chart in page.charts {
            let bundle = store.get_bundle(chart.bundle_id)?;
            // Do what you need to with the chart data at bundle.chart
            // and retain chart.bundle_id on your chart type.
        }

        if !has_more {
            break;
        }
        offset += page_len;
    }
    ```

=== "C"

    ```c
    const char *extensions[] = { "bms", "bme", "bml", "bmson" };
    uint64_t offset = 0;
    bool has_more;
    do {
        bkb_bundle_search_result *page = NULL;
        bkb_store_search_bundles(store, NULL, offset, 100, extensions, 4, &page);

        for (size_t i = 0; i < page->charts_len; ++i) {
            const bkb_bundle_search_chart *chart = &page->charts[i];
            // Open chart->bundle_id with bkb_store_get_bundle, then parse bb->chart.
            // Keep chart->bundle_id on your chart type.
        }

        size_t page_len = page->charts_len;
        has_more = page->has_more;
        bkb_bundle_search_result_free(page);
        offset += page_len;
    } while (has_more);
    ```

=== "Java"

    ```java
    long offset = 0;
    boolean hasMore;
    do {
        BundleSearchResult page = store.searchBundles(null, offset, 100,
                "bms", "bme", "bml", "bmson");

        for (BundleSearchChart chart : page.charts()) {
            try (BackbeatFile bundle = store.getBundle(chart.bundleId()).orElseThrow()) {
                // Parse bundle.chartData() and keep chart.bundleId() on your chart type.
            }
        }

        offset += page.charts().size();
        hasMore = page.hasMore();
    } while (hasMore);
    ```

Use the page's `has_more`/`hasMore` field and a larger offset to fetch the next page.
Handle errors from the SDK calls in production code.

## 5. Adding packs, tables, courses

Collections are how backbeat lets people organise charts. Use the SDK to
list and read packs, courses, and difficulty tables:

=== "Rust"

    ```rust
    let packs = store.list_packs(&[])?;
    let pack = store.get_pack(&packs[0].url)?;
    ```

=== "C"

    ```c
    bkb_collection_metadata_list *packs = NULL;
    bkb_store_list_packs(store, NULL, 0, &packs);
    // Read pack URLs from packs, then use bkb_store_get_pack for their contents.
    bkb_collection_metadata_list_free(packs);
    ```

=== "Java"

    ```java
    List<CollectionMetadata> packs = store.listPacks();
    PackContents pack = store.getPack(packs.get(0).url()).orElseThrow();
    ```

The above example is for packs, but it's the same for courses and tables.

What your game _does_ with this information is up to your game. If you already have a nice abstraction
for packs, courses or tables, just feed/map this data into that. If you don't, you'll have to build something.

## 6. (Extra) Doing powerful queries

Backbeat uses a sqlite database to organise itself. The schema for that database is considered part of the public API.

The details are in [backbeat store](backbeat-store).

By using the `attach` api, you can attach backbeat to your sqlite database, and then query `backbeat.$table_name` however you like. **Please be aware that you get a READ-ONLY connection. It is expressly ILLEGAL to make direct writes to the backbeat.db, and doing this will almost certainly result in database corruption.**

=== "Rust"

    ```rust
    let attach = store.sqlite_attach_command();
    // Execute attach on your SQLite connection.
    ```

=== "C"

    ```c
    bkb_string attach = {0};
    error = bkb_store_sqlite_attach_command(store, &attach);
    /* Execute attach.ptr on your SQLite connection. */
    bkb_string_free(attach);
    ```

=== "Java"

    ```java
    String attach = store.sqliteAttachCommand();
    // Execute attach on your SQLite connection.
    ```

This attaches the Backbeat database to your sqlite connection as `backbeat` for read-only queries.
For example, you could construct powerful joins like this:

```sql
SELECT id, filename, extension, description
FROM
	backbeat.bundle
JOIN
	your_game_charts ON backbeat.bundle.chart_sha256 = your_game_charts.sha256
WHERE
	backbeat.bundle.extension IN ('bms', 'sm');
```

## Reference Integrations

Of course, you can just look at how other games have implemented Backbeat and copy that.
Here's some of the reference PRs we raised to get Backbeat into games.

- TODO
