# Priority in GitHub Issues

Verification date: August 7, 2026.

## Conclusion

GitHub now allows `Priority` to be stored directly on an Issue through **organization-level Issue Fields**, without the Issue belonging to a Project. At the time of this research, this looked like the most natural source because the value lives on the Issue and is consistent across all Projects.

The decisive limitation is that Issue Fields are only available for repositories owned by an **organization**. They do not work in repositories owned by personal accounts. For those repositories, Hyfa would need a fallback—for example, labels—or would have to declare them out of scope.

GitHub announced general availability on July 2, 2026 for all organizations on Free, Team, Enterprise, and GitHub Enterprise Cloud with data residency; it will arrive in GitHub Enterprise Server 3.23. [Official general availability announcement](https://github.blog/changelog/2026-07-02-issue-fields-are-now-generally-available/)

## How it is assigned in the interface

Each organization receives a `Priority` single-select field with these options by default:

- `Urgent`
- `High`
- `Medium`
- `Low`

The organization owner can modify the name, order, and options—for example, changing them to `P0`–`P4`—under **Organization → Settings → Planning → Issue fields**. The field can be pinned to specific Issue types or to untyped Issues. [Managing Issue Fields in the organization](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/managing-issue-fields-in-your-organization)

To assign it to an Issue:

1. Open the Issue.
2. In the right sidebar, click **Add field** if `Priority` is not visible.
3. Select `Priority` and then an option.

The change is saved automatically. People with `triage` access or higher can edit values. Currently, it cannot be prepopulated through a URL parameter or an issue template. Pull Requests do not support Issue Fields. [Assigning and editing Issue Field values](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-and-managing-issue-fields)

## Issue Field versus Project Field

| Mechanism | Where the value lives | Scope | Consequence for Hyfa |
| --- | --- | --- | --- |
| Organization Issue Field | On the Issue | All repositories in the same organization | Recommended; no Project required |
| Project custom field | On a Project item | One specific Project | The same Issue can have different priorities in different Projects |
| Label | On the Issue, within the repository | One repository | Universal fallback, but without typing or mutual exclusion |

GitHub explicitly documents that Issue Fields are the source of truth for an Issue, while a Project Field is limited to the Project and can have different values for the same Issue. [Official difference between Issue Fields and Project Fields](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/managing-issue-fields-in-your-organization#issue-fields-and-projects)

If a Project Field is used, the Issue must first be a Project item. It can be edited with GraphQL `updateProjectV2ItemFieldValue` or the Projects REST API.

## REST and GraphQL

### Organization Issue Fields

The `2026-03-10` REST API provides, among others, these endpoints:

- `GET /orgs/{org}/issue-fields`: discover the field and its options.
- `GET /repos/{owner}/{repo}/issues/{number}/issue-field-values`: read an Issue's values.
- `POST /repos/{owner}/{repo}/issues/{number}/issue-field-values`: add or update specified values.
- `PUT /repos/{owner}/{repo}/issues/{number}/issue-field-values`: replace all existing values.
- `DELETE /repos/{owner}/{repo}/issues/{number}/issue-field-values/{field_id}`: delete a value.

For a single-select, the field ID and the option's exact name are sent:

```json
{
  "issue_field_values": [
    { "field_id": 123, "value": "High" }
  ]
}
```

Normal Issue creation and update also accept `issue_field_values`. Reading requires `Issues: read`; writing requires `Issues: write`, and GitHub documents that setting values through the API requires `push` access. Managing the field definition requires the organization permission `Issue Fields` and administrator access to create or modify it. [REST for Issue Field values](https://docs.github.com/en/rest/issues/issue-field-values), [REST for Issue Field definitions](https://docs.github.com/en/rest/orgs/issue-fields), [REST for Issues](https://docs.github.com/en/rest/issues/issues?apiVersion=2026-03-10)

GraphQL exposes `Issue.issueFieldValues`, the `IssueFieldSingleSelect`/`IssueFieldSingleSelectValue` types, `createIssue` with `issueFields`, and the `setIssueFieldValue` mutation. This allows Issues and their priority to be fetched in a nested form and avoids the REST pattern of one additional request per Issue. [GraphQL schema for Issues](https://docs.github.com/en/graphql/reference/issues)

The REST Issue listing supports filtering with `issue_field_values=priority:High` and combining it with `since`; it does not include a complete collection of field values in each object in the documented response. For synchronizing thousands of Issues, nested GraphQL or a few REST queries per option are preferable to `GET issue-field-values` for each Issue.

## Fallback with labels

For a personal-account repository or a GitHub Enterprise Server version earlier than 3.23, labels can be defined as:

```text
priority:p0
priority:p1
priority:p2
priority:p3
priority:p4
```

They work in any repository and are already included in the REST Issue listing. However, GitHub does not prevent an Issue from having multiple priority labels simultaneously; Hyfa would have to detect that state as a conflict and not silently choose one of them. [Labels REST API](https://docs.github.com/en/rest/issues/labels)

## Initial recommendation for Hyfa v1 (superseded)

This recommendation predates ADR 0021. Hyfa v1 ultimately chose canonical `priority:p0` through `priority:p4` labels as its only priority source so organization-owned and personal repositories share one implementation. The API findings above remain useful background; the list below records the earlier proposal and is not normative.

1. Use a configurable single-select Organization Issue Field, named `Priority` by default, as the primary source.
2. Discover the field and its options; do not hard-code only the English names, because GitHub allows them to be customized. The configured order of the options should define the priority order.
3. Do not depend on Projects to obtain priority. If a Project Field is supported, treat it as an explicit alternative source; never automatically combine it with the Issue Field of the same name.
4. Explicitly decide whether v1 excludes personal repositories or supports `priority:*` labels as a fallback. If labels are supported, require zero or one priority label per Issue and report conflicts.
