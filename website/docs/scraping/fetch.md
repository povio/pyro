# One-off fetches (CLI)

`pyro fetch` loads one URL, runs the page's scripts, and prints the resulting DOM as HTML. `pyro eval` runs JavaScript in a blank page. Both are single commands with no server and no client library, so they fit shell scripts and cron jobs.

Use them for quick jobs. For sign-in, clicking, proxies, custom headers or many pages, use a script instead: see [Scripting a browser](/scraping/library).

## `fetch`

```sh
pyro fetch [--dump html|none] [--wait-ms N] [--timeout-ms N] URL
```

If you installed the npm package locally, run it as `npx pyro` or `bunx pyro`.

| Flag | Default | What it does |
| --- | --- | --- |
| `URL` | required | The page to load. One URL per run. |
| `--dump html` | yes | Print the DOM after the page loaded, as `<!DOCTYPE html>` plus the `<html>` element's outer HTML. |
| `--dump none` | | Load the page and print nothing. Useful to check that a page loads, or to see its console output. |
| `--wait-ms N` | `0` | After `load`, keep running the page until it is idle or `N` ms have passed, whichever comes first. |
| `--timeout-ms N` | `30000` | Deadline for the whole run, the `load` and the wait included. Running JavaScript is stopped when it passes. |

How a run goes:

1. The browser navigates to `URL`, running the page's scripts as they load.
2. It waits for the `load` event.
3. With `--wait-ms`, it keeps running timers, `fetch` callbacks and other tasks until nothing is left to do, or until `N` ms have passed.
4. It prints the DOM and exits.

### `--wait-ms` matters for JavaScript-rendered pages

Without `--wait-ms`, the DOM is printed right at `load`. Data a page fetches after `load` is not there yet. A test page that starts a `fetch` and a 200 ms timer from an inline script:

```sh
$ pyro fetch http://127.0.0.1:4500/late | grep -c 'id="fetched"'
0
$ pyro fetch --wait-ms 1000 http://127.0.0.1:4500/late | grep -c 'id="fetched"'
1
```

`--wait-ms` is a cap, not a sleep. The run ends as soon as the page is idle: `--wait-ms 5000` on that page finished in 0.6 s. A page that never goes idle (polling, a ticking clock) runs until the cap. Set the cap to the longest you are willing to wait.

### Output and exit codes

The HTML goes to stdout. The page's `console.log` and `console.info` go to stdout too, and `console.warn`, `console.error`, uncaught exceptions and errors go to stderr. Keep that in mind when you pipe stdout: a page that logs to the console mixes its log lines into the HTML. Use `--dump none` to see only the console output. If the reading end of a pipe closes early (`| head`), `fetch` fails to write and prints a Rust panic message to stderr; the output you did read is complete up to that point.

| Exit code | Meaning |
| --- | --- |
| `0` | The page loaded and the DOM was printed. A page whose own scripts throw still exits `0`. |
| `1` | The navigation failed (`net::ERR_CONNECTION_REFUSED`, DNS failure and so on), with the reason on stderr. |
| `2` | Bad arguments. |
| `124` | The deadline passed before `load` or during the wait. |

### What `fetch` does not have

`fetch` uses the browser's defaults and has no other options:

- no proxy, cookies, extra headers or user agent flags (the user agent is `Mozilla/5.0 (compatible; pyro/<version>)`)
- no way to click, type or wait for a selector
- no screenshot or PDF output
- one URL per process

For any of those, start `pyro serve` and drive it from a script.

## `eval`

```sh
pyro eval [--timeout-ms N] (-e CODE | FILE | -)
```

`eval` runs JavaScript in an `about:blank` page until the event loop is idle. Pass the code with `-e`, as a file path, or on stdin with `-`.

| Flag | Default | What it does |
| --- | --- | --- |
| `-e CODE` | | Run `CODE`. |
| `FILE` | | Run the file. |
| `-` | | Read the code from stdin. |
| `--timeout-ms N` | `30000` | Stop the code after `N` ms and exit with `124`. |

`console.log` goes to stdout, `console.error` to stderr. Uncaught exceptions and unhandled promise rejections are printed to stderr and make the exit code `1`.

```sh
$ pyro eval -e 'console.log(navigator.userAgent)'
Mozilla/5.0 (compatible; pyro/X.Y.Z)
$ echo 'console.log(JSON.stringify({ ok: 1 }))' | pyro eval -
{"ok":1}
```

::: warning eval has no network
The `eval` page has no network. `fetch()` rejects with `Page networking is not installed`. Use `eval` to test JavaScript behaviour of the engine, not to scrape. To run code against a loaded page, use `page.evaluate()` from a script.
:::

## Piping into other tools

`fetch` prints HTML, so any HTML tool can read it. The examples use a local test site at `http://127.0.0.1:4500`; replace it with your URL.

Extract with [pup](https://github.com/ericchiang/pup) or [htmlq](https://github.com/mgdm/htmlq):

```sh
pyro fetch --wait-ms 2000 'http://127.0.0.1:4500/products?page=1' \
  | htmlq --text '.product .name'
```

Convert to text or Markdown with [pandoc](https://pandoc.org/):

```sh
pyro fetch --wait-ms 2000 https://example.com/article \
  | pandoc -f html -t gfm-raw_html -o article.md
```

Parse it in Python with BeautifulSoup (`pip install beautifulsoup4`):

```sh
pyro fetch --wait-ms 2000 'http://127.0.0.1:4500/products?page=1' \
  | python3 -c 'import sys, bs4; print([li.select_one(".name").text for li in bs4.BeautifulSoup(sys.stdin, "html.parser").select(".product")])'
```

Quote URLs that contain `?` or `&`. In zsh an unquoted `?` is a glob and the command fails with `no matches found`.

## Shell loops

One process per URL is simple and isolates pages from each other. Each run starts a fresh engine, so there is no shared state between URLs.

A sequential crawl of numbered pages with a polite delay:

```sh
mkdir -p pages
for page in $(seq 1 5); do
  url="http://127.0.0.1:4500/products?page=$page"
  if pyro fetch --wait-ms 2000 --timeout-ms 20000 "$url" > "pages/$page.html"; then
    echo "ok   $url"
  else
    echo "fail $url (exit $?)" >&2
    rm -f "pages/$page.html"
  fi
  sleep 1
done
```

A list of URLs, four at a time, with `xargs`:

```sh
# urls.txt: one URL per line
mkdir -p out
cat -n urls.txt | xargs -P 4 -L 1 sh -c \
  'pyro fetch --wait-ms 2000 --timeout-ms 20000 "$1" > "out/$0.html" || { rm -f "out/$0.html"; echo "failed: $1" >&2; }'
```

`-P 4` runs four browsers at once. Keep the number low for a single site; see [Scraping responsibly](/scraping/overview#scraping-responsibly).

Retry once on a timeout:

```sh
fetch_retry() {
  pyro fetch --wait-ms 2000 --timeout-ms 20000 "$1" && return
  [ $? -eq 124 ] && sleep 5 && pyro fetch --wait-ms 2000 --timeout-ms 40000 "$1"
}
fetch_retry 'http://127.0.0.1:4500/products?page=1' > page1.html
```

## When to move to a script

A process per URL pays for engine start and the page's boot every time, and it cannot keep cookies between URLs. Move to `pyro serve` and a script when you need sign-in, many pages from one site, cookies, headers, proxies, clicking, or PDFs. [Scripting a browser](/scraping/library) shows the same crawl as a script. For every CLI command and flag, see the [CLI reference](/reference/cli).
