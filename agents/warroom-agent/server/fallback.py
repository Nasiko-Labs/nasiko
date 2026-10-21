import json
from pathlib import Path
from server.schemas import CompanyContext, CompetitorConfig

DATA_FILE = Path(__file__).resolve().parent.parent / "data" / "company_context.json"


def load_default_company_context() -> tuple[CompanyContext, list[CompetitorConfig]]:
    """Load default PayFlow demo company profile and monitored competitors list.

    Note: This contains configuration data only, NOT fabricated external intelligence.
    """
    if not DATA_FILE.exists():
        # Sensible fallback defaults matching specification
        company = CompanyContext(
            name="PayFlow",
            products=["Payment Gateway", "Reconciliation", "FraudShield"],
            target_customers=["SMB", "Mid-Market"],
        )
        competitors = [
            CompetitorConfig(name="Cashfree"),
            CompetitorConfig(name="Razorpay"),
            CompetitorConfig(name="PayU"),
        ]
        return company, competitors

    with open(DATA_FILE, "r", encoding="utf-8") as f:
        data = json.load(f)

    company = CompanyContext(
        name=data.get("name", "PayFlow"),
        products=data.get("products", ["Payment Gateway", "Reconciliation", "FraudShield"]),
        target_customers=data.get("target_customers", ["SMB", "Mid-Market"]),
    )

    raw_competitors = data.get("competitors", ["Cashfree", "Razorpay", "PayU"])
    competitors = [CompetitorConfig(name=c) for c in raw_competitors]

    return company, competitors
