import React, { useState, useEffect } from "react";
import { Sidebar, ActivePage } from "./components/Sidebar";
import { TopHeader } from "./components/TopHeader";
import { OverviewView } from "./components/OverviewView";
import { SignalsView } from "./components/SignalsView";
import { CompetitorsView } from "./components/CompetitorsView";
import { ResponsePlansView } from "./components/ResponsePlansView";
import { ResearchView } from "./components/ResearchView";
import { EvidenceView } from "./components/EvidenceView";
import { AskWarroomView } from "./components/AskWarroomView";
import { IntegrationsView } from "./components/IntegrationsView";
import { SignalDetailModal } from "./components/SignalDetailModal";
import { CompetitiveSignal, ScanResponse } from "./types";
import { scanLandscape, checkHealth } from "./api";

export const App: React.FC = () => {
  const [activePage, setActivePage] = useState<ActivePage>("overview");
  const [scanData, setScanData] = useState<ScanResponse | null>(null);
  const [isScanning, setIsScanning] = useState(false);
  const [lastScanTime, setLastScanTime] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedSignalForModal, setSelectedSignalForModal] = useState<CompetitiveSignal | null>(null);
  const [selectedSignalForSim, setSelectedSignalForSim] = useState<CompetitiveSignal | null>(null);
  const [globalError, setGlobalError] = useState<string | null>(null);

  // Check health on load
  useEffect(() => {
    checkHealth().catch(() => {
      // Backend not yet ready or offline
    });
  }, []);

  const handleScan = async () => {
    setIsScanning(true);
    setGlobalError(null);
    try {
      const data = await scanLandscape();
      setScanData(data);
      const now = new Date();
      const timeStr = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      setLastScanTime(`Today, ${timeStr}`);
    } catch (err: any) {
      setGlobalError(err.message || "Failed to scan competitive landscape.");
    } finally {
      setIsScanning(false);
    }
  };

  const handleOpenSignalModal = (signal: CompetitiveSignal) => {
    setSelectedSignalForModal(signal);
  };

  const handleSimulateSignal = (signal: CompetitiveSignal) => {
    setSelectedSignalForSim(signal);
    setActivePage("response-plans");
  };

  const signals = scanData?.signals || [];
  const research = scanData?.research || [];
  const company = scanData?.company || {
    name: "PayFlow",
    products: ["Payment Gateway", "Reconciliation", "FraudShield"],
    target_customers: ["SMB", "Mid-Market"],
  };

  return (
    <div className="app-container">
      {/* Persistent Left Sidebar */}
      <Sidebar
        activePage={activePage}
        onSelectPage={setActivePage}
        signalCount={signals.length}
      />

      {/* Main Content Area */}
      <div className="main-wrapper">
        <TopHeader
          companyName={company.name}
          isScanning={isScanning}
          onScan={handleScan}
          lastScanTime={lastScanTime}
          searchQuery={searchQuery}
          onSearchChange={setSearchQuery}
        />

        <main className="page-container">
          {globalError && (
            <div
              style={{
                padding: "12px 18px",
                backgroundColor: "var(--danger-light)",
                color: "var(--danger)",
                borderRadius: "var(--radius-md)",
                marginBottom: 20,
                fontSize: "0.85rem",
                border: "1px solid var(--danger-border)",
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
              }}
            >
              <span>{globalError}</span>
              <button
                className="secondary-btn"
                style={{ fontSize: "0.75rem", padding: "2px 8px", height: 26 }}
                onClick={handleScan}
              >
                Retry
              </button>
            </div>
          )}

          {activePage === "overview" && (
            <OverviewView
              scanData={scanData}
              onScan={handleScan}
              isScanning={isScanning}
              onSelectSignal={handleOpenSignalModal}
              onNavigate={setActivePage}
            />
          )}

          {activePage === "signals" && (
            <SignalsView
              signals={signals}
              onSelectSignal={handleOpenSignalModal}
            />
          )}

          {activePage === "competitors" && (
            <CompetitorsView
              signals={signals}
              research={research}
              onSelectSignal={handleOpenSignalModal}
            />
          )}

          {activePage === "response-plans" && (
            <ResponsePlansView
              signals={signals}
              selectedSignal={selectedSignalForSim}
              onSelectSignal={setSelectedSignalForSim}
            />
          )}

          {activePage === "research" && (
            <ResearchView research={research} />
          )}

          {activePage === "evidence" && (
            <EvidenceView
              signals={signals}
              research={research}
              onSelectSignal={handleOpenSignalModal}
            />
          )}

          {activePage === "ask" && (
            <AskWarroomView signals={signals} />
          )}

          {(activePage === "integrations" || activePage === "settings") && (
            <IntegrationsView company={company} />
          )}
        </main>
      </div>

      {/* Signal Detail Modal */}
      <SignalDetailModal
        signal={selectedSignalForModal}
        onClose={() => setSelectedSignalForModal(null)}
        onSimulate={handleSimulateSignal}
      />
    </div>
  );
};
