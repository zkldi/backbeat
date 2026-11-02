# Backbeat

Backbeat is a standard for getting and storing rhythm game charts.

It can be trivially integrated with any game, frequently has faster performance than the filesystem for games like BMS, and de-duplicates files to save disk space and download times.

**If you're just someone that plays rhythm games, you can install [the Backbeat App](https://backbeat.ac) and then click `Download with Backbeat` buttons on websites.** Beyond that you shouldn't need to know or care about Backbeat - everything else here is technical crap for nerds.

It also comes with support for Packs, Difficulty Tables and Courses, which are ways to arrange rhythm game charts. Games that integrate Backbeat get these powerful features, _for free_.

You never again have to faff around with google drive links, trawling discord, and unraring files in the right locale.

[Read more at the official website.](https://backbeat.ac)

## What it is not.

Backbeat is not a Rhythm Game, and it's not a replacement for a "Songs" folder on your PC.

**Games that integrate with Backbeat MUST keep their support for their regular read-game-content-from-disk, as it is absolutely necessary for content creators.**

It has **absolutely no support** for editing content, by design, so it will _never_ be possible for a chart editor to sensibly work with it. Backbeat is intended for distributing **finished, immutable content.**

## Games that support Backbeat:

- All of them. Backbeat is game-agnostic, and works with any game that has been written, or will be written. The protocol has no game-specific logic.

## Clients that integrate Backbeat:

- [RhythmGame](https://rhythmgame.eu/)
- Tentatively, lr2oraja-endlessdream
- Tentatively, unnamed-sdvx-clone
- (Your game, here?)

## Backbeat Servers:

- [Makiba](https://makiba.ac)

Makiba is the largest chart database ever made, and it speaks Backbeat. Just add `https://makiba.ac` to your list of data servers.

- (Your server, here?)
