export async function run(h) {
  const held = chrome.userScripts;
  await h.host("disableUserScripts");
  let denied = false;
  for(let i=0;i<100;i++) {
    try { await held.getScripts(); } catch(error) {
      if(error.message === "'userScripts.getScripts' is not available in this context.") { denied=true;break; }
      throw error;
    }
    await h.sleep(10);
  }
  h.assert(denied,"user scripts capability was not revoked");
  for(const method of ["getScripts","register","configureWorld","execute","getWorldConfigurations","resetWorldConfiguration","unregister","update"]) {
    let callback = false;
    try {
      held[method](Symbol(),()=>{callback=true});
      h.record(method,{returned:true,callback});
    } catch(error) {
      h.record(method,{sync:true,name:error.name,message:error.message,callback});
    }
  }
}
