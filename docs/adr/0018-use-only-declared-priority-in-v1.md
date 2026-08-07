# Use only declared priority in v1

Grit v1 will use Declared priority as its only human-supplied ranking field. It will not require, read, or infer impact, effort, urgency, or risk for ranking. After readiness and availability gates, `grit next` will combine this ordinal input with the calculated Unlock profile under ADR 0028; Structural centrality remains secondary. An Issue without a usable Declared priority will remain eligible and neutral rather than being treated as low value. This keeps the first version compatible with repositories that maintain little structured metadata.
