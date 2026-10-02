import subprocess
import sys
import time

def main():
    print("Starting GrantMatch AI Agents...")
    
    # Define the agents and their ports
    agents = [
        {"name": "Grant Scout", "dir": "grant_scout", "port": 8001},
        {"name": "Compliance Analyst", "dir": "compliance_analyst", "port": 8002},
        {"name": "Synthesis", "dir": "synthesis", "port": 8003},
    ]
    
    processes = []
    
    for agent in agents:
        print(f"Starting {agent['name']} on port {agent['port']}...")
        cmd = [sys.executable, "-m", "uvicorn", f"{agent['dir']}.main:app", "--host", "0.0.0.0", "--port", str(agent['port'])]
        
        # Start the process in the background
        p = subprocess.Popen(cmd)
        processes.append((agent['name'], p))
        
        # Give it a second to start
        time.sleep(1)
        
    print("\nAll agents started! Press Ctrl+C to stop.")
    
    try:
        # Keep the main thread alive
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        print("\nStopping all agents...")
        for name, p in processes:
            print(f"Terminating {name}...")
            p.terminate()
            p.wait()
        print("All agents stopped.")

if __name__ == "__main__":
    main()
