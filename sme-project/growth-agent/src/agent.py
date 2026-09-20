import os
import re
import pandas as pd
from pathlib import Path
from collections.abc import AsyncIterator
from openai import AsyncOpenAI

DATA_DIR = Path(__file__).parent / "data"


def fmt_inr(amount: float) -> str:
    """Format a rupee amount in Indian style: Rs X.XX Cr or Rs X.XX L."""
    abs_amt = abs(amount)
    sign = "-" if amount < 0 else ""
    if abs_amt >= 1_00_00_000:  # >= 1 crore
        return f"{sign}Rs {abs_amt / 1_00_00_000:.2f} Cr"
    elif abs_amt >= 1_00_000:   # >= 1 lakh
        return f"{sign}Rs {abs_amt / 1_00_000:.2f} L"
    else:
        return f"{sign}Rs {abs_amt:,.2f}"


def get_data_period(sales_df: pd.DataFrame) -> str:
    """Extract first and last date from sales_raw.csv."""
    date_cols = [c for c in sales_df.columns if 'date' in c.lower() or 'Date' in c]
    if not date_cols:
        return "Data period: unknown (no date column found)"
    col = date_cols[0]
    try:
        dates = pd.to_datetime(sales_df[col], errors='coerce').dropna()
        if dates.empty:
            return "Data period: unknown (no valid dates)"
        return f"Data period: {dates.min().strftime('%d %b %Y')} to {dates.max().strftime('%d %b %Y')}"
    except Exception:
        return "Data period: unknown"


def simulate_scenario(price_change_pct: float, cost_change_pct: float, price_elasticity: float = -1.2) -> str:
    """Simulates business revenue and profit changes based on price/cost change % and price elasticity."""
    try:
        sales_path = DATA_DIR / "sales_raw.csv"
        po_path = DATA_DIR / "purchase_orders.csv"
        exp_path = DATA_DIR / "expenses.csv"

        if not sales_path.exists():
            return f"Error: Missing file {sales_path.name}"
        if not po_path.exists():
            return f"Error: Missing file {po_path.name}"
        if not exp_path.exists():
            return f"Error: Missing file {exp_path.name}"

        sales_df = pd.read_csv(sales_path)
        po_df = pd.read_csv(po_path)
        exp_df = pd.read_csv(exp_path)

        if 'TotalAmount' not in sales_df.columns:
            return "Error: Missing column 'TotalAmount' in sales_raw.csv"
        if 'TotalAmountSpent' not in po_df.columns:
            return "Error: Missing column 'TotalAmountSpent' in purchase_orders.csv"
        if 'AmountSpent' not in exp_df.columns:
            return "Error: Missing column 'AmountSpent' in expenses.csv"

        # Baseline
        sales_df['TotalAmount'] = pd.to_numeric(sales_df['TotalAmount'], errors='coerce')
        current_revenue = sales_df['TotalAmount'].sum()

        po_df['TotalAmountSpent'] = pd.to_numeric(po_df['TotalAmountSpent'], errors='coerce')
        current_cogs = po_df['TotalAmountSpent'].sum()

        exp_df['AmountSpent'] = pd.to_numeric(exp_df['AmountSpent'], errors='coerce')
        current_opex = exp_df['AmountSpent'].sum()

        current_profit = current_revenue - current_cogs - current_opex

        # Projections
        volume_change_pct = price_change_pct * price_elasticity
        volume_multiplier = 1 + (volume_change_pct / 100)

        projected_revenue = current_revenue * (1 + price_change_pct / 100) * volume_multiplier
        projected_cogs = current_cogs * (1 + cost_change_pct / 100) * volume_multiplier
        projected_profit = projected_revenue - projected_cogs - current_opex
        profit_delta = projected_profit - current_profit

        period = get_data_period(sales_df)

        result = "[DATA BLOCK]\n"
        result += f"{period}\n\n"
        result += "Assumptions:\n"
        result += f"- Price Change: {price_change_pct:+.1f}%\n"
        result += f"- Cost Change: {cost_change_pct:+.1f}%\n"
        result += f"- Price Elasticity: {price_elasticity} → Volume changes by {volume_change_pct:+.2f}%\n"
        result += "- Operating expenses remain constant.\n\n"

        result += "Current Baseline:\n"
        result += f"  - Revenue : {fmt_inr(current_revenue)}\n"
        result += f"  - COGS    : {fmt_inr(current_cogs)}\n"
        result += f"  - OPEX    : {fmt_inr(current_opex)}\n"
        result += f"  - Profit  : {fmt_inr(current_profit)}\n\n"

        result += "Projected Metrics (after scenario):\n"
        result += f"  - Revenue : {fmt_inr(projected_revenue)}\n"
        result += f"  - COGS    : {fmt_inr(projected_cogs)}\n"
        result += f"  - Profit  : {fmt_inr(projected_profit)} (Δ {fmt_inr(profit_delta)})\n"

        return result
    except Exception as e:
        return f"Error simulating scenario: {str(e)}"


class GrowthAgent:
    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        self._client = AsyncOpenAI(
            api_key=os.getenv("OPENAI_API_KEY", "not-needed-if-using-local-router"),
            base_url=os.getenv("OPENAI_BASE_URL", "http://host.docker.internal:8080/v1"),
        )
        self._model_name = os.getenv("MODEL") or os.getenv("ROUTER_MODEL") or "gpt-4o-mini"

    def _parse_query(self, query: str) -> tuple[float, float]:
        query_lower = query.lower()
        price_change_pct = 0.0
        cost_change_pct = 0.0

        # Match patterns like "raise prices by 5%", "lower costs by 3%"
        matches = list(re.finditer(
            r'(raise|increase|lower|decrease|cut)\s+(?:the\s+)?(?:prices?|costs?|expenses?)\s*(?:by)?\s*(\d+(?:\.\d+)?)\s*%?',
            query_lower
        ))
        for match in matches:
            action = match.group(1)
            val = float(match.group(2))
            context = query_lower[max(0, match.start()-20): match.end()+20]
            multiplier = -1 if action in ['lower', 'decrease', 'cut'] else 1
            if 'price' in context:
                price_change_pct = val * multiplier
            elif 'cost' in context or 'expense' in context:
                cost_change_pct = val * multiplier

        # Fallback: any % number + price/cost keyword
        if price_change_pct == 0 and cost_change_pct == 0:
            nums = re.findall(r'(\d+(?:\.\d+)?)\s*%', query_lower)
            if nums:
                val = float(nums[0])
                direction = -1 if any(w in query_lower for w in ['lower', 'decrease', 'cut', 'reduce']) else 1
                if 'price' in query_lower or 'prices' in query_lower:
                    price_change_pct = val * direction
                elif 'cost' in query_lower or 'costs' in query_lower:
                    cost_change_pct = val * direction

        return price_change_pct, cost_change_pct

    async def invoke(self, query: str, context_id: str) -> str:
        res = ""
        async for chunk in self.invoke_streaming(query, context_id):
            res += chunk
        return res

    async def invoke_streaming(self, query: str, context_id: str) -> AsyncIterator[str]:
        price_change, cost_change = self._parse_query(query)
        data_block = simulate_scenario(price_change, cost_change)

        if "Error:" in data_block:
            yield data_block
            return

        try:
            stream = await self._client.chat.completions.create(
                model=self._model_name,
                messages=[
                    {"role": "system", "content": (
                        "You are a financial summary assistant. "
                        "The DATA BLOCK contains Python-computed numbers from real business data. "
                        "Answer the user's exact question in 3 short sentences using the numbers in the DATA BLOCK. "
                        "State the baseline profit, projected profit, and the profit change in rupees. "
                        "Do NOT add numbers of your own. Do NOT write generic economics. Do NOT mention elasticity theory."
                    )},
                    {"role": "user", "content": f"Question: {query}\n\n{data_block}"},
                ],
                stream=True,
            )

            yield f"{data_block}\n\n"

            async for chunk in stream:
                delta = chunk.choices[0].delta if chunk.choices else None
                if delta and delta.content:
                    yield delta.content
        except Exception as e:
            yield f"{data_block}\n\n[Note: LLM formatting failed ({str(e)}). Above is the raw computed result.]"
