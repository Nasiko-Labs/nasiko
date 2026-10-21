"""Cost accounting independent of cloud pricing or inference implementations."""
from dataclasses import dataclass
from decimal import Decimal


@dataclass(frozen=True)
class RunCost:
    """All amounts cover the same run window, including startup and drain."""

    compute: Decimal
    storage_and_network: Decimal
    decisions: Decimal
    successful_workflows: int
    failed_workflows: int
    pending_workflows: int

    def __post_init__(self) -> None:
        for amount in (self.compute, self.storage_and_network, self.decisions):
            if not isinstance(amount, Decimal) or not amount.is_finite() or amount < 0:
                raise ValueError("Costs must be finite nonnegative Decimal amounts")
        counts = (self.successful_workflows, self.failed_workflows, self.pending_workflows)
        if any(type(count) is not int or count < 0 for count in counts):
            raise ValueError("Workflow counts must be nonnegative integers")

    @property
    def total(self) -> Decimal:
        return self.compute + self.storage_and_network + self.decisions

    @property
    def cost_per_success(self) -> Decimal | None:
        """Failures consume cost; a run without success has no finite unit cost."""
        if self.successful_workflows == 0:
            return None
        return self.total / self.successful_workflows
