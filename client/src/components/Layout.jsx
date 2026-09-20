import { NavLink, Outlet, useLocation } from 'react-router-dom';
import { LayoutDashboard, Users, UserPlus, Phone, Zap } from 'lucide-react';

const navItems = [
  { path: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { path: '/leads', icon: Users, label: 'Leads' },
  { path: '/leads/new', icon: UserPlus, label: 'Add Lead' },
  { path: '/calls', icon: Phone, label: 'Call History' },
];

export default function Layout() {
  const location = useLocation();

  return (
    <div className="min-h-screen flex">
      {/* Sidebar */}
      <aside className="w-64 bg-white border-r border-border flex flex-col fixed h-full z-20">
        {/* Logo */}
        <div className="h-16 flex items-center gap-3 px-6 border-b border-border">
          <div className="w-9 h-9 rounded-xl bg-gradient-to-br from-nova-500 to-nova-700 flex items-center justify-center shadow-lg shadow-nova-500/25">
            <Zap className="w-5 h-5 text-white" />
          </div>
          <div>
            <h1 className="text-lg font-bold text-slate-900 tracking-tight">NOVA</h1>
            <p className="text-[10px] font-medium text-nova-500 uppercase tracking-widest -mt-0.5">CRM</p>
          </div>
        </div>

        {/* Navigation */}
        <nav className="flex-1 px-3 py-4 space-y-1">
          {navItems.map(({ path, icon: Icon, label }) => {
            const isActive = path === '/' ? location.pathname === '/' : location.pathname.startsWith(path);
            return (
              <NavLink
                key={path}
                to={path}
                className={`flex items-center gap-3 px-3 py-2.5 rounded-xl text-sm font-medium transition-all duration-200 group
                  ${isActive
                    ? 'bg-nova-50 text-nova-700 shadow-sm'
                    : 'text-slate-500 hover:text-slate-900 hover:bg-slate-50'
                  }`}
              >
                <Icon className={`w-[18px] h-[18px] ${isActive ? 'text-nova-600' : 'text-slate-400 group-hover:text-slate-600'}`} />
                {label}
              </NavLink>
            );
          })}
        </nav>

        {/* Bottom branding */}
        <div className="p-4 border-t border-border">
          <div className="bg-gradient-to-br from-nova-50 to-nova-100/50 rounded-xl p-3">
            <p className="text-xs font-semibold text-nova-700">AI Sales Dialer</p>
            <p className="text-[11px] text-nova-500 mt-0.5">Powered by NOVA Voice Agent</p>
          </div>
        </div>
      </aside>

      {/* Main Content */}
      <main className="flex-1 ml-64">
        {/* Top bar */}
        <header className="h-16 bg-white/80 backdrop-blur-md border-b border-border sticky top-0 z-10 flex items-center px-8">
          <div className="flex items-center gap-2 text-sm text-slate-400">
            <span className="w-2 h-2 rounded-full bg-accent-green animate-pulse"></span>
            <span>System Online</span>
          </div>
        </header>

        {/* Page Content */}
        <div className="p-8">
          <Outlet />
        </div>
      </main>
    </div>
  );
}
