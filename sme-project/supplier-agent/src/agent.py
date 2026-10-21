import os
import re
import pandas as pd
from pathlib import Path
from collections.abc import AsyncIterator
from openai import AsyncOpenAI

DATA_DIR = Path(__file__).parent / "data"


def fmt_inr(amount: float) -> str:
    abs_amt = abs(amount)
    sign = "-" if amount < 0 else ""
    if abs_amt >= 1_00_00_000:
        return f"{sign}Rs {abs_amt / 1_00_00_000:.2f} Cr"
    elif abs_amt >= 1_00_000:
        return f"{sign}Rs {abs_amt / 1_00_000:.2f} L"
    else:
        return f"{sign}Rs {abs_amt:,.2f}"


def get_data_period(df: pd.DataFrame) -> str:
    date_cols = [c for c in df.columns if 'date' in c.lower()]
    if not date_cols:
        return "Data period: unknown"
    try:
        dates = pd.to_datetime(df[date_cols[0]], errors='coerce').dropna()
        if dates.empty:
            return "Data period: unknown"
        return f"{dates.min().strftime('%d %b %Y')} to {dates.max().strftime('%d %b %Y')}"
    except Exception:
        return "Data period: unknown"


def build_facts() -> str:
    try:
        sup_path = DATA_DIR / "suppliers.csv"
        po_path  = DATA_DIR / "purchase_orders.csv"
        sales_path = DATA_DIR / "sales_raw.csv"

        if not sup_path.exists():
            return f"Error: Missing file {sup_path.name}"
        if not po_path.exists():
            return f"Error: Missing file {po_path.name}"

        suppliers_df = pd.read_csv(sup_path)
        po_df = pd.read_csv(po_path)

        po_df["TotalAmountSpent"] = pd.to_numeric(po_df["TotalAmountSpent"], errors="coerce")
        po_df["CostPerUnit"]      = pd.to_numeric(po_df["CostPerUnit"],      errors="coerce")
        po_df["QuantityOrdered"]  = pd.to_numeric(po_df["QuantityOrdered"],  errors="coerce")
        suppliers_df["ReliabilityScore"] = pd.to_numeric(suppliers_df["ReliabilityScore"], errors="coerce")

        # ── Spend per supplier ────────────────────────────────────────────────
        po_spend = po_df.groupby("SupplierID")["TotalAmountSpent"].sum().reset_index()
        po_spend.columns = ["SupplierID", "TotalSpend"]
        merged = pd.merge(suppliers_df, po_spend, on="SupplierID", how="left")
        merged["TotalSpend"] = merged["TotalSpend"].fillna(0)

        active = merged[merged["TotalSpend"] > 0]
        n_active = len(active)
        total_spend = active["TotalSpend"].sum()
        avg_reliability = active["ReliabilityScore"].mean()

        # Data period
        period = "unknown"
        if sales_path.exists():
            sales_df = pd.read_csv(sales_path)
            period = get_data_period(sales_df)

        out  = "[FACTS BLOCK]\n"
        out += f"Note: This data records purchases from suppliers. 'Sales' in this context means spend/purchases.\n"
        out += f"Data period: {period}\n\n"
        out += f"Overview:\n"
        out += f"  - Suppliers with purchase orders: {n_active}\n"
        out += f"  - Total spend across all suppliers: {fmt_inr(total_spend)}\n"
        out += f"  - Average reliability score: {avg_reliability:.1f}/10\n\n"

        # ── Top 5 by spend ────────────────────────────────────────────────────
        top5 = active.sort_values("TotalSpend", ascending=False).head(5)
        out += "Top 5 Suppliers by Spend:\n"
        for _, r in top5.iterrows():
            out += f"  - {r['SupplierName']} (ID: {r['SupplierID']}): {fmt_inr(r['TotalSpend'])}, Reliability {r['ReliabilityScore']}/10\n"

        # ── Bottom 5 by spend ─────────────────────────────────────────────────
        bot5 = active.sort_values("TotalSpend", ascending=True).head(5)
        out += "\nBottom 5 Suppliers by Spend (lowest):\n"
        for _, r in bot5.iterrows():
            out += f"  - {r['SupplierName']} (ID: {r['SupplierID']}): {fmt_inr(r['TotalSpend'])}, Reliability {r['ReliabilityScore']}/10\n"

        # ── Lowest reliability (risk) ─────────────────────────────────────────
        risk5 = active.sort_values("ReliabilityScore", ascending=True).head(5)
        at_risk_spend = active[active["ReliabilityScore"] < 6]["TotalSpend"].sum()
        out += "\n5 Lowest-Reliability Suppliers:\n"
        for _, r in risk5.iterrows():
            flag = " ⚠ HIGH RISK" if r["ReliabilityScore"] < 6 else ""
            out += f"  - {r['SupplierName']}: Reliability {r['ReliabilityScore']}/10, Spend {fmt_inr(r['TotalSpend'])}{flag}\n"
        out += f"  Total spend at risk (reliability < 6): {fmt_inr(at_risk_spend)}\n"

        # ── Savings candidates ────────────────────────────────────────────────
        candidates = active[active["ReliabilityScore"] >= 7].copy()
        candidates["s5"] = candidates["TotalSpend"] * 0.05
        candidates["s8"] = candidates["TotalSpend"] * 0.08
        candidates = candidates.sort_values("s8", ascending=False).head(5)
        out += "\nTop 5 Savings Candidates (negotiation assumption — 5% and 8% NOT measured from data):\n"
        for _, r in candidates.iterrows():
            out += (f"  - {r['SupplierName']} (ID: {r['SupplierID']}): "
                    f"Spend {fmt_inr(r['TotalSpend'])} → "
                    f"save {fmt_inr(r['s5'])} (5%) to {fmt_inr(r['s8'])} (8%)\n")

        return out
    except Exception as e:
        return f"Error building supplier facts: {e}"


class SupplierAgent:
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
        facts = build_facts()

        if facts.startswith("Error:"):
            yield facts
            return

        llm_answer = ""
        try:
            response = await self._client.chat.completions.create(
                model=self._model_name,
                messages=[
                    {"role": "system", "content": (
                        "You are a procurement advisor. "
                        "Answer ONLY the question asked. "
                        "Use only numbers that appear in the FACTS block. "
                        "Write 3 to 4 short sentences in Indian rupees. "
                        "Do not invent numbers. "
                        "Describe negotiation percentages as assumptions. "
                        "If the FACTS cannot answer the question, say what the data can answer instead."
                    )},
                    {"role": "user", "content": f"Question: {query}\n\n{facts}"},
                ],
            )
            llm_answer = response.choices[0].message.content or ""
        except Exception as e:
            llm_answer = f"[LLM unavailable: {e}]"

        yield llm_answer + "\n\n---\nData used:\n" + facts
