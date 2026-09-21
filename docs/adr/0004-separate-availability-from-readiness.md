# Separate availability from readiness

Hyfa will treat dependency readiness and human ownership as separate states: an assigned Issue may be Ready but is not Available as new work. `hyfa next` will rank unassigned Ready Issues by default, while an explicit assignee filter may rank that person's Ready Issues; this avoids recommending work already owned without misrepresenting assignment as a dependency.
