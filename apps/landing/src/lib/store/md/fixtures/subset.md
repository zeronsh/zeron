[![build](https://ci.example.com/badge.svg)](https://ci.example.com/runs) [![license](https://img.example.com/mit.svg)](LICENSE)

Cost meter
==========

Shows what each chat costs, *as it goes*, in the sidebar. **No setup**, ~~no account~~; it reads your
agents' own usage.  
A hard break, and a `code span` with `` ` `` in it.

## Install

1. Open **Settings** › Plugins.
2. Find *Cost meter*.
3. Install it:
   ```sh
   zeron plugin install cost-meter
   ```

## Settings

| Setting | What it does | Default |
|:--------|:------------:|--------:|
| `limit` | The monthly limit, in dollars. | 50 |
| `warn` | When to warn you \| a share of the limit. | 0.8 |
| `currency` | | USD |

## To do

- [x] Per-chat totals
- [ ] Per-project totals
  - grouped by week
  - grouped by month
- [ ] Exports

> **Note**
> It never sends your usage anywhere.
>
> > Quoted twice.

Loose list:

* one

* two

    indented code inside nothing

***

See [the guide][guide], [Settings][], and [the changelog](CHANGELOG.md#changes) or [a heading](#install).
Mail <help@example.com> or open <https://example.com/issues>.

A picture of its own: ![The meter](shots/meter.png "In the sidebar")

<details><summary>Raw HTML</summary> is its characters &amp; entities &copy; are what they say.</details>

Escapes: \*not emphasis\*, \[not a link\], 1\. not a list.

[guide]: https://example.com/guide "The guide"
[settings]: https://example.com/settings
