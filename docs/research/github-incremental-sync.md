# Incremental synchronization of GitHub Issues

**Date:** 2026-08-06  
**Scope:** a local replica of the Issues and native dependencies of a single repository.

## Conclusion

The Issue number is useful for identifying new Issues, but it is not a change cursor. An old Issue can be closed, assigned, edited, or have its labels changed without receiving a new number.

Hyfa can maintain the ordinary part of the replica incrementally using `updated_at`, but it cannot use that same signal as a guarantee for dependencies. The practical strategy requires two streams:

1. New or modified Issues since a temporal watermark.
2. Relationship events since an independent checkpoint.

Without a durable webhook, GitHub does not offer a local CLI a dependency change feed with a formally guaranteed cursor, ordering, and retention. Therefore, a full reconciliation remains necessary whenever Hyfa cannot demonstrate continuity.

## New and modified Issues

The [`GET /repos/{owner}/{repo}/issues`](https://docs.github.com/en/rest/issues/issues?apiVersion=2026-03-10#list-repository-issues) endpoint supports:

- `state=all`, which is necessary to observe closures and reopenings;
- `sort=updated` and `direction`;
- `since=<timestamp>`, which returns items updated after the specified instant;
- up to 100 items per page.

The REST endpoint also returns pull requests; Hyfa must exclude objects that contain `pull_request`. GraphQL avoids that mixture: [`Repository.issues`](https://docs.github.com/en/graphql/reference/repos#repository) supports `IssueFilters.since`, ordering by `UPDATED_AT`, and cursor-based pagination of up to 100 nodes.

A delta must not begin at the highest known number, but at a temporal watermark maintained internally. To account for temporal precision, ties, and races during pagination, an overlapping window and idempotent upserts by stable identity must be used.

GitHub does not provide snapshot isolation while pages are traversed. Its [REST best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api) warn that sorting by update time can reposition items during pagination. The watermark must advance only after the pass has been completed and verified.

## Unchanged requests

GitHub recommends conditional requests with `ETag`/`If-None-Match`. An authenticated `304 Not Modified` response does not consume primary rate limit quota, according to its [REST best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api).

The ETag validates a specific URL and representation. It does not replace the watermark or prove that other pages or endpoints have not changed.

## Dependencies require another signal

[`Issue.updatedAt`](https://docs.github.com/en/graphql/reference/issues#issue) is documented only as the last update to the object. GitHub does not guarantee that adding or removing a dependency relationship changes that field.

The live API confirms that at least one addition can occur without advancing `Issue.updatedAt`: during this research, `BlockedByAddedEvent` events were observed after the `updatedAt` of the affected Issues. This is evidence of behavior, not an additional contract.

GitHub does document four exact timeline events:

- `BlockedByAddedEvent`;
- `BlockedByRemovedEvent`;
- `BlockingAddedEvent`;
- `BlockingRemovedEvent`.

They can be queried through [`Issue.timelineItems`](https://docs.github.com/en/graphql/reference/issues#issue), which supports filtering by type and `since`, but that connection belongs to each Issue: it is not a global repository feed.

There is also a dedicated [`issue_dependencies`](https://docs.github.com/en/webhooks/webhook-events-and-payloads#issue_dependencies) webhook, with the four actions and both ends of the relationship. It is the precise push signal, but it requires a durable receiver and is no longer a purely local solution.

## Repository event feed

[`GET /repos/{owner}/{repo}/issues/events`](https://docs.github.com/en/rest/issues/events#list-issue-events-for-a-repository) lists repository events. The current API returns dependency events, and the [official OpenAPI specification](https://github.com/github/rest-api-description/blob/e50419c4bb8f2d1d34735044bb3b410863dc0a10/descriptions/api.github.com/api.github.com.2026-03-10.yaml#L46934-L46980) includes the `blocked_by` and `blocking` fields.

However, the endpoint reference promises only pagination through `page` and `per_page`. It does not document:

- a `since` parameter;
- a resume cursor;
- the order of the results;
- history retention.

Persisting IDs, paginating until an already known event is found, overlapping, and deduplicating provide a practical delta. By themselves, they do not provide a lossless guarantee after an arbitrary interruption.

## Deletions and transfers

The incremental listing does not emit tombstones. A request for an individual Issue can return `301` for a transfer, `410` for a visible deletion, or `404` for a deletion/transfer without access, according to [`GET .../issues/{issue_number}`](https://docs.github.com/en/rest/issues/issues?apiVersion=2026-03-10#get-an-issue).

GitHub exposes `deleted` and `transferred` actions in the Issues webhook, but a polling-only replica needs to reconcile the inventory to detect absences reliably.

## Recommended algorithm for Hyfa

### Initial run

1. Fetch the complete relevant inventory.
2. Fetch `blockedBy` for each open Issue; one direction is enough to build the graph.
3. Save the technical watermarks, checkpoints, and ETags.
4. Publish the replica only when the complete operation has finished successfully.

### Before `next`, `plan`, `triage`, or `graph`

1. Conditionally revalidate the Issues view and the first page of events.
2. If the Issues changed, fetch `state=all` from the watermark with overlap and apply idempotent upserts.
3. If the events changed, paginate and deduplicate until the known checkpoint is found; apply only one orientation of each dependency event to avoid duplicating edges.
4. If an unknown reference appears, fetch that Issue before closing the transaction.
5. If the checkpoint is not found, an incompatible event appears, or continuity cannot be verified, perform a full reconciliation instead of declaring the delta complete.
6. Replace the replica atomically and update `synced_at` only after all streams have completed.
7. If GitHub is unavailable, retain the previous replica and compute from it.

With no changes, the usual path can be reduced to conditional requests that return `304`. With few changes, the cost depends on the delta rather than the total number of Issues.

## Achievable guarantee

Ordinary synchronization can be efficient and detect that someone assigned themselves an Issue without traversing old numbers: that Issue reappears in the update delta. Relationships are updated through the event feed.

The combination is **best-effort with loss-of-continuity detection**, not a formally exact replica forever. Accuracy after an arbitrary pause requires one of these two things:

- full reconciliation of the current state; or
- durable, idempotent, and auditable webhooks, while still maintaining repair reconciliations because GitHub does not automatically retry all failed deliveries.

For a general-purpose CLI without a server, full reconciliation as the repair path is the option compatible with the proposed product.
