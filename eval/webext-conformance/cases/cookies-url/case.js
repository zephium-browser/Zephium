export async function run(h) {
  const url = 'https://127.0.0.1/account/page';
  const base = {url, name:'run4-cookie',value:'test-only',secure:true,httpOnly:true,sameSite:'strict',path:'/'};
  h.record('set', await chrome.cookies.set(base));
  h.record('get', await chrome.cookies.get({url,name:base.name}));
  h.record('callback', await new Promise(resolve => chrome.cookies.get({url,name:base.name}, resolve)));
  await chrome.cookies.set({...base,path:'/account',value:'longer-path'});
  h.record('longest path', await chrome.cookies.get({url,name:base.name}));
  h.record('all', (await chrome.cookies.getAll({url,name:base.name})).sort((a,b)=>a.path.localeCompare(b.path)));
  h.record('other host', await chrome.cookies.get({url:'https://localhost/',name:base.name}));
  h.record('stores shape', (await chrome.cookies.getAllCookieStores()).map(s=>({keys:Object.keys(s).sort(),id:s.id,tabIds:s.tabIds.every(Number.isInteger)})));
  h.record('remove', await chrome.cookies.remove({url,name:base.name}));
  h.record('after remove', await chrome.cookies.getAll({url,name:base.name}));
  h.record('missing', await chrome.cookies.remove({url,name:base.name}));
  for (const details of [{...base,name:'__Host-token',secure:false},{...base,domain:'com'},{...base,domain:'localhost'}]) {
    let refused=false; try { await chrome.cookies.set(details); } catch { refused=true; }
    h.record('invalid refused', refused);
  }
}
