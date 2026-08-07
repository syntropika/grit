# Bound lookahead planning

Grit may recommend the first step of a multi-step plan, but it will never search all possible plans. Lookahead will be bounded by an explicit Planning horizon, candidate shortlist, beam width, and time budget; when a limit is reached, Grit will return the best executable plan found and disclose that the search was truncated, keeping runtime predictable on repositories with thousands of Issues at the cost of not claiming global optimality.
