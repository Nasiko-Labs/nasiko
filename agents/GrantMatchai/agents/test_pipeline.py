import httpx
import json

def test():
    abstract = "Project AEGIS: AI-driven disaster response and triage coordination platform using swarm intelligence and edge vision models on drones."
    
    print("1. Querying Grant Scout (Port 8001)...")
    res1 = httpx.post("http://localhost:8001/run", json={"task_id": "t1", "abstract": abstract}, timeout=30.0)
    print("Status code:", res1.status_code)
    print("Response text:", res1.text)
    scout_out = res1.json().get("result")
    print("Grant Scout Result:\n", scout_out)
    
    print("\n2. Querying Compliance Analyst (Port 8002)...")
    res2 = httpx.post("http://localhost:8002/run", json={"task_id": "t1", "abstract": abstract, "grant_target": str(scout_out)}, timeout=30.0)
    comp_out = res2.json().get("result")
    print("Compliance Analyst Result:\n", comp_out)
    
    print("\n3. Querying Synthesis Agent (Port 8003)...")
    res3 = httpx.post("http://localhost:8003/run", json={"task_id": "t1", "abstract": abstract, "grants": str(scout_out), "compliance": str(comp_out)}, timeout=30.0)
    syn_out = res3.json().get("result")
    print("\n=== FINAL SYNTHESIS OUTPUT ===")
    print(syn_out)

if __name__ == "__main__":
    test()
