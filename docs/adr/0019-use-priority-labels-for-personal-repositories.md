---
status: superseded by ADR-0021
---

# Use priority labels for personal repositories

Hyfa v1 will obtain Declared priority from an organization-level Issue Field when the Repository belongs to a GitHub organization. Because GitHub does not provide Issue Fields to repositories owned by personal accounts, Hyfa will also recognize the canonical labels `priority:p0` through `priority:p4` there, with P0 as the highest priority. A missing field value or priority label means unspecified priority and does not make an Issue ineligible. Hyfa will select the representation from the Repository ownership context instead of merging Issue Field and label values, preserving one authoritative priority source per analysis while supporting personal repositories without Projects. If a personal-repository Issue has more than one canonical priority label, Hyfa will report a Priority conflict and ignore all of its declared priority values; the Issue remains eligible and is still evaluated using calculated graph signals.
