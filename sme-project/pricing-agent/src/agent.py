import os
import pandas as pd
from pathlib import Path
from collections.abc import AsyncIterator
from openai import AsyncOpenAI

DATA_DIR = Path(__file__).parent / "data"

MARGIN_MIN = -20.0   # sanity lower bound
MARGIN_MAX = 90.0    # sanity upper bound
PRICE_ELASTICITY = -1.2


def fmt_inr(amount: float) -> str:
    """Format a rupee amount in Indian style: Rs X.XX Cr or Rs X.XX L."""
    abs_amt = abs(amount)
    sign = "-" if amount < 0 else ""
    if abs_amt >= 1_00_00_000:
        return f"{sign}Rs {abs_amt / 1_00_00_000:.2f} Cr"
    elif abs_amt >= 1_00_000:
        return f"{sign}Rs {abs_amt / 1_00_000:.2f} L"
    else:
        return f"{sign}Rs {abs_amt:,.2f}"


def get_data_period(sales_df: pd.DataFrame) -> str:
    date_cols = [c for c in sales_df.columns if 'date' in c.lower()]
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


def project_effect(current_price: float, new_price: float,
                   current_volume: float, unit_cost: float,
                   elasticity: float = PRICE_ELASTICITY,
                   label: str = "Projected effect") -> str:
    """Compute projected volume change % and profit change using price elasticity."""
    price_change_pct = (new_price - current_price) / current_price * 100
    volume_change_pct = price_change_pct * elasticity
    new_volume = current_volume * (1 + volume_change_pct / 100)
    old_profit = (current_price - unit_cost) * current_volume
    new_profit = (new_price - unit_cost) * new_volume
    profit_delta = new_profit - old_profit
    return (
        f"  {label} (elasticity {elasticity}): "
        f"volume {volume_change_pct:+.1f}% → profit change {fmt_inr(profit_delta)}"
    )


def get_pricing_strategy() -> str:
    """
    Analyse products and sales to compute unit margins and suggest capped price ranges.

    Cost source: products.csv → DerivedCost = DefaultSellingPrice / StandardMarkup
    (StandardMarkup is the retail markup factor, e.g. 1.20 means 20% gross margin.)
    purchase_orders.CostPerUnit is a bulk/batch procurement cost and is NOT used
    for margin because it is not comparable to the per-unit retail selling price.
    """
    try:
        prod_path = DATA_DIR / "products.csv"
        sales_path = DATA_DIR / "sales_raw.csv"

        if not prod_path.exists():
            return f"Error: Missing file {prod_path.name}"
        if not sales_path.exists():
            return f"Error: Missing file {sales_path.name}"

        products_df = pd.read_csv(prod_path)
        sales_df = pd.read_csv(sales_path)

        required_prod_cols = {'ProductID', 'ProductName', 'DefaultSellingPrice', 'StandardMarkup'}
        missing = required_prod_cols - set(products_df.columns)
        if missing:
            return f"Error: Missing columns in products.csv: {missing}"
        if 'Quantity' not in sales_df.columns:
            return "Error: Missing column 'Quantity' in sales_raw.csv"

        period = get_data_period(sales_df)

        # ── Unit cost via StandardMarkup ─────────────────────────────────────
        # StandardMarkup = SellingPrice / CostPrice → CostPrice = Price / Markup
        products_df['DefaultSellingPrice'] = pd.to_numeric(products_df['DefaultSellingPrice'], errors='coerce')
        products_df['StandardMarkup'] = pd.to_numeric(products_df['StandardMarkup'], errors='coerce')
        products_df['DerivedCost'] = products_df['DefaultSellingPrice'] / products_df['StandardMarkup']
        products_df['MarginPct'] = (
            (products_df['DefaultSellingPrice'] - products_df['DerivedCost'])
            / products_df['DefaultSellingPrice'] * 100
        )

        # ── Sales Volume ──────────────────────────────────────────────────────
        sales_df['Quantity'] = pd.to_numeric(sales_df['Quantity'], errors='coerce')
        sales_vol = sales_df.groupby('ProductID')['Quantity'].sum().reset_index()
        sales_vol.columns = ['ProductID', 'TotalSold']

        merged = pd.merge(products_df, sales_vol, on='ProductID', how='left')
        merged['TotalSold'] = merged['TotalSold'].fillna(0)
        top_products = merged.sort_values(by='TotalSold', ascending=False).head(5)

        result = f"[DATA BLOCK]\n{period}\n"
        result += "Cost derived from StandardMarkup (DefaultSellingPrice / StandardMarkup)\n\n"
        result += "Pricing Analysis for Top 5 Products by Volume:\n"

        for _, row in top_products.iterrows():
            margin = row['MarginPct']
            sold = int(row['TotalSold'])
            price = row['DefaultSellingPrice']
            cost = row['DerivedCost']

            result += f"\n- {row['ProductName']}\n"
            result += f"  Current Price : {fmt_inr(price)} | Cost : {fmt_inr(cost)} | Margin : {margin:.1f}% | Volume : {sold:,} units\n"

            # ── Sanity check ─────────────────────────────────────────────────
            if margin < MARGIN_MIN or margin > MARGIN_MAX:
                result += "  Margin        : cost data unreliable for this product\n"
                continue

            # ── Capped price increase bands ──────────────────────────────────
            # margin < 10%  → allow up to +10%
            # all others    → cap at +3% (low end) to +8% (high end)
            if margin < 10:
                pct_lo, pct_hi = 5.0, 10.0
            elif margin < 15:
                pct_lo, pct_hi = 3.0, 8.0
            else:
                # Healthy margin — nudge range
                pct_lo, pct_hi = 3.0, 5.0

            suggested_lo = price * (1 + pct_lo / 100)
            suggested_hi = price * (1 + pct_hi / 100)

            result += f"  Suggested Range: {fmt_inr(suggested_lo)} (+{pct_lo:.0f}%) to {fmt_inr(suggested_hi)} (+{pct_hi:.0f}%)\n"

            # ── Projected effect at BOTH ends of the range ───────────────────
            result += project_effect(price, suggested_lo, sold, cost,
                                     label=f"At {fmt_inr(suggested_lo)} (+{pct_lo:.0f}%)") + "\n"
            result += project_effect(price, suggested_hi, sold, cost,
                                     label=f"At {fmt_inr(suggested_hi)} (+{pct_hi:.0f}%)") + "\n"

        return result
    except Exception as e:
        return f"Error analyzing pricing: {str(e)}"


class PricingAgent:
    SUPPORTED_CONTENT_TYPES = ["text", "text/plain"]

    def __init__(self):
        self._client = AsyncOpenAI(
            api_key=os.getenv("OPENAI_API_KEY", "not-needed-if-using-local-router"),
            base_url=os.getenv("OPENAI_BASE_URL", "http://host.docker.internal:8080/v1"),
        )
        self._model_name = os.getenv("MODEL") or os.getenv("ROUTER_MODEL") or "gpt-4o-mini"

    async def invoke(self, query: str, context_id: str) -> str:
        res = ""
        async for chunk in self.invoke_streaming(query, context_id):
            res += chunk
        return res

    async def invoke_streaming(self, query: str, context_id: str) -> AsyncIterator[str]:
        data_block = get_pricing_strategy()

        if "Error:" in data_block:
            yield data_block
            return

        yield f"{data_block}\n\n"

        try:
            stream = await self._client.chat.completions.create(
                model=self._model_name,
                messages=[
                    {"role": "system", "content": (
                        "You are a pricing analyst assistant. "
                        "The DATA BLOCK contains Python-computed numbers from real CSV data. "
                        "Answer the user's exact question in 3 short sentences. "
                        "Mention the specific product, its suggested price range, and the projected profit change. "
                        "Use ONLY the numbers from the DATA BLOCK — do NOT add numbers of your own. "
                        "Do NOT write generic economics."
                    )},
                    {"role": "user", "content": f"Question: {query}\n\n{data_block}"},
                ],
                stream=True,
            )

            async for chunk in stream:
                delta = chunk.choices[0].delta if chunk.choices else None
                if delta and delta.content:
                    yield delta.content
        except Exception as e:
            yield f"[Note: LLM formatting failed ({str(e)}). Above is the raw computed result.]"

