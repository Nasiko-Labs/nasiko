let _frameworksCache = null;
let _ownersCache = null;

window.filterFrameworks = async (query) => {
  if (!_frameworksCache) {
    try {
      const res = await fetch('/v1/meta/frameworks');
      const { data } = await res.json();
      _frameworksCache = data.map(f => ({ label: f, value: f }));
    } catch { _frameworksCache = []; }
  }
  if (!query) return _frameworksCache;
  const q = query.toLowerCase();
  return _frameworksCache.filter(f => f.value.includes(q));
};

window.filterOwners = async (query) => {
  if (!_ownersCache) {
    try {
      const res = await fetch('/v1/meta/owners');
      const { data } = await res.json();
      _ownersCache = data.map(o => ({ label: o, value: o }));
    } catch { _ownersCache = []; }
  }
  if (!query) return _ownersCache;
  const q = query.toLowerCase();
  return _ownersCache.filter(o => o.value.includes(q));
};

function getAdminAuth() {
  let creds = sessionStorage.getItem('registry_admin_creds');
  if (!creds) {
    const user = prompt('Admin username:');
    if (!user) throw new Error('Authentication cancelled');
    const pass = prompt('Admin password:');
    if (!pass) throw new Error('Authentication cancelled');
    creds = btoa(`${user}:${pass}`);
    sessionStorage.setItem('registry_admin_creds', creds);
  }
  return `Basic ${creds}`;
}

window.fetchArtifacts = async (query, page, limit) => {
  try {
    const params = new URLSearchParams({ limit });
    if (query) params.set('q', query);
    const res = await fetch(`/v1/search?${params}`);
    if (!res.ok) throw new Error(res.statusText);
    const data = await res.json();
    return { data: data.data || [], total: data.total || 0 };
  } catch { return { data: [], total: 0 }; }
};

window.fetchAgentArtifacts = async (query, page, limit) => {
  try {
    const params = new URLSearchParams({ limit, type: 'agent' });
    if (query) params.set('q', query);
    const res = await fetch(`/v1/search?${params}`);
    if (!res.ok) throw new Error(res.statusText);
    const data = await res.json();
    return { data: data.data || [], total: data.total || 0 };
  } catch { return { data: [], total: 0 }; }
};

window.fetchSkillArtifacts = async (query, page, limit) => {
  try {
    const params = new URLSearchParams({ limit, type: 'skill' });
    if (query) params.set('q', query);
    const res = await fetch(`/v1/search?${params}`);
    if (!res.ok) throw new Error(res.statusText);
    const data = await res.json();
    return { data: data.data || [], total: data.total || 0 };
  } catch { return { data: [], total: 0 }; }
};

window.yankArtifact = async (owner, name, version) => {
  const auth = getAdminAuth();
  const res = await fetch(`/v1/artifacts/${owner}/${name}/${version}`, {
    method: 'DELETE',
    headers: { 'Authorization': auth },
  });
  if (res.status === 401) {
    sessionStorage.removeItem('registry_admin_creds');
    throw new Error('Invalid credentials');
  }
  if (!res.ok) throw new Error(await res.text());
  return res.json();
};

window.publishArtifact = async (payload) => {
  const auth = getAdminAuth();
  const res = await fetch('/v1/artifacts', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': auth,
    },
    body: JSON.stringify(payload),
  });
  if (res.status === 401) {
    sessionStorage.removeItem('registry_admin_creds');
    throw new Error('Invalid credentials');
  }
  if (!res.ok) throw new Error(await res.text());
  return res.json();
};
