# excelerate in the browser

The browser build lands in `npm/pkg-web`, the Node build in `npm/pkg`. They are
not interchangeable, which is why the folders differ:

```
tools/build-npm.sh web        # -> npm/pkg-web
tools/build-npm.sh            # -> npm/pkg, target nodejs
tools/build-npm.sh bundler    # -> npm/pkg-bundler, for webpack and the like
```

Opening `index.html` by double-clicking it will not work: over `file://` the
browser refuses to load modules. Any http server will do:

```
cd npm && python3 -m http.server 8731
```

and `http://localhost:8731/browser/index.html`.

There is exactly one difference from Node - the module has to be initialized
first:

```js
import init, { Book } from "../pkg-web/excelerate.js";
await init();               // fetches and starts the .wasm
const book = new Book();
```

After that the API is the same: `set`, `get`, `evaluate`, `registerFunction`,
`toXlsx`, `Book.read`, `cellStyle` and the rest.

The `.wasm` file arrives as a separate resource, and the server should serve it
with `Content-Type: application/wasm` - then the browser compiles it as it
streams. Python's `http.server` does; if yours does not, `init()` still works,
only slower.
