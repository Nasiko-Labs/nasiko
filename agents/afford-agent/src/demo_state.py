"""Hardcoded demo spending profile for the live Build-A-Thon demo."""

from datetime import date, timedelta

_TODAY = date.today()


def demo_state() -> dict:
    return {
        "current_balance": 42000.0,
        "minimum_balance_to_keep": 10000.0,
        "confirmed_recurring_income": [
            {
                "event_id": "sal-001",
                "event_type": "salary",
                "direction": "credit",
                "category": "income",
                "description": "Monthly salary",
                "currency": "INR",
                "event_date": (_TODAY - timedelta(days=15)).isoformat(),
                "amount_home_currency": 60000.0,
                "recurrence_interval_days": 30,
                "status": "settled",
            }
        ],
        "confirmed_recurring_expenses": [
            {
                "event_id": "rent-001",
                "event_type": "rent",
                "direction": "debit",
                "category": "housing",
                "description": "Monthly rent",
                "currency": "INR",
                "event_date": (_TODAY - timedelta(days=15)).isoformat(),
                "amount_home_currency": 18000.0,
                "recurrence_interval_days": 30,
                "status": "settled",
            },
            {
                "event_id": "emi-001",
                "event_type": "loan_emi",
                "direction": "debit",
                "category": "debt",
                "description": "Laptop EMI",
                "currency": "INR",
                "event_date": (_TODAY - timedelta(days=10)).isoformat(),
                "amount_home_currency": 4500.0,
                "recurrence_interval_days": 30,
                "status": "settled",
            },
        ],
        "events": [],
        "one_time_events": [],
        "preferences": {
            "payment_methods_user_will_consider": [
                "full_payment",
                "partial_payment",
                "installments",
            ],
            "expense_categories_to_protect": [],
            "expense_categories_user_is_willing_to_reduce": [],
            "expense_categories_user_is_willing_to_stop": [],
            "max_installment_months": 6,
        },
    }