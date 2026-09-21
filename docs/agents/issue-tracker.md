# Issue tracker

Hyfa tracks product work in GitHub Issues in
[`syntropika/hyfa`](https://github.com/syntropika/hyfa/issues).

Use Hyfa with the explicit repository selector so commands do not depend on
the current checkout. Inspect individual Issues in the generated explorer
or directly on GitHub:

```bash
hyfa sync --repo syntropika/hyfa
hyfa graph --repo syntropika/hyfa --output site/
```

Implementation commits reference their originating Issue number. The
`ready-for-agent` label means the Issue specification is ready to implement;
native `blocked by` relationships define the executable frontier.
