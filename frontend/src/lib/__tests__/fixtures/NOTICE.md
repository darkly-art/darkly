# Test-fixture fonts

## Cantarell-VF.woff2

The WOFF2-compressed form of `crates/darkly/tests/fixtures/fonts/Cantarell-VF.otf`,
generated with `woff2-encoder`'s `compress` for the real-decoder test
(`woff2_decode.test.ts`). Regenerate with:

```
node --input-type=module -e "import {compress} from 'woff2-encoder';\
import {readFileSync,writeFileSync} from 'node:fs';\
writeFileSync('src/lib/__tests__/fixtures/Cantarell-VF.woff2',\
await compress(new Uint8Array(readFileSync('../crates/darkly/tests/fixtures/fonts/Cantarell-VF.otf'))))"
```

- **Family:** Cantarell · **License:** SIL Open Font License 1.1 · © The Cantarell Authors
- **Source:** <https://gitlab.gnome.org/GNOME/cantarell-fonts>
