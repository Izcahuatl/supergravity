# Figura Libraries

This folder hosts Lua libraries for
[Figura](https://figuramc.org/) avatars.

## How to add a new library

1. Drop your `.lua` file into `figura/libs/`.
2. Open `figura/libraries.json` and add an entry to the `libraries` array:

```json
{
  "libraries": [
    {
      "id": "myLib",
      "name": "My Library",
      "version": "1.0.0",
      "description": "One-sentence summary of what it does.",
      "filename": "myLib.lua",
      "doc": "<p>Short usage docs. Supports HTML.</p><pre><code>local myLib = require('myLib')</code></pre>"
    }
  ]
}
```

3. Refresh `figura/index.html` to see the new card.

## Fields

- `id` — URL-safe identifier (not currently shown).
- `name` — Display name of the library.
- `version` — Version string (optional, defaults to `1.0.0`).
- `description` — Short summary.
- `filename` — Name of the `.lua` file inside `figura/libs/`.
- `doc` — Short documentation; HTML is allowed.
