"""The nodes of the lane flow, as `rules` and `rule_agents` both need them."""

NODES = ("start", "plan", "plan_review", "build", "task_review", "final_review", "checks", "deliver", "guard")
REVIEW_NODES = ("plan_review", "task_review", "final_review")
