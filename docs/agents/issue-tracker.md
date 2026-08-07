# Issue tracker

Grit tracks product work in GitHub Issues in
[`syntropika/grit`](https://github.com/syntropika/grit/issues).

Use the `gh` CLI with the explicit repository selector so commands do not
depend on the current checkout:

```bash
gh issue view ISSUE --repo syntropika/grit
```

Implementation commits reference their originating Issue number. The
`ready-for-agent` label means the Issue specification is ready to implement;
native `blocked by` relationships define the executable frontier.
