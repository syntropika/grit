# Issue tracker

Grit tracks product work in GitHub Issues in
[`syntropika/grit`](https://github.com/syntropika/grit/issues).

Use Grit with the explicit repository selector so commands do not depend on
the current checkout. Inspect individual Issues in the generated explorer
or directly on GitHub:

```bash
grit sync --repo syntropika/grit
grit graph --repo syntropika/grit --output site/
```

Implementation commits reference their originating Issue number. The
`ready-for-agent` label means the Issue specification is ready to implement;
native `blocked by` relationships define the executable frontier.
