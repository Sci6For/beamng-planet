//! Session-bound audio commands; never execute command text supplied by the game.
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_fresh_bounded_session_commands() {
        let good=br#"{"version":1,"session":"current","sequence":3,"command":"ping"}"#;
        assert_eq!(accepted_sequence(good,"current",2),Some(3));
        assert_eq!(accepted_sequence(good,"old",2),None);
        assert_eq!(accepted_sequence(good,"current",3),None);
        assert_eq!(accepted_sequence(&vec![b' ';16385],"current",0),None);
    }
    #[test]
    fn failed_worker_retries_unacknowledged_but_not_completed_command() {
        assert_eq!(recovery_sequence(8,Some(7)),7);
        assert_eq!(recovery_sequence(8,Some(8)),8);
        assert_eq!(recovery_sequence(8,None),7);
    }
    #[test]
    fn owned_worker_acknowledges_command_and_is_stopped() {
        let directory=std::env::temp_dir().join(format!("planet-audio-test-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&directory).unwrap();
        let directory=directory.canonicalize().unwrap();
        let mut runtime=AudioRuntime::default();runtime.tick(&directory,"test");
        fs::write(directory.join("audio-request.json"),br#"{"version":1,"session":"test","sequence":1,"command":"ping"}"#).unwrap();
        let start=Instant::now();let mut ack=false;
        while start.elapsed()<Duration::from_secs(15) {
            runtime.tick(&directory,"test");
            if let Ok(b)=fs::read(directory.join("audio-status.json")) {
                if let Ok(v)=serde_json::from_slice::<serde_json::Value>(&b) {if v["ready"]==true && v["sequence"]==1 {ack=true;break}}
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        runtime.stop();
        assert!(runtime.child.is_none());
        assert!(ack,"worker did not acknowledge ping: {:?}",fs::read_to_string(directory.join("audio-status.json")));
        let _=fs::remove_dir_all(directory);
    }
}
fn recovery_sequence(sent:u64,ack:Option<u64>)->u64 {
    if ack.is_some_and(|v|v>=sent){sent}else{sent.saturating_sub(1)}
}
// Config canonicalization returns verbatim paths. Windows PowerShell's FileSystem
// provider (Join-Path, Get-Item) cannot handle that prefix even though Win32 can.
fn powershell_path(path:&Path)->io::Result<std::path::PathBuf> {
    let text=path.to_str().ok_or_else(||io::Error::new(io::ErrorKind::InvalidInput,"audio path is not valid Unicode"))?;
    Ok(if let Some(unc)=text.strip_prefix(r"\\?\UNC\") {
        std::path::PathBuf::from(format!(r"\\{unc}"))
    } else {
        std::path::PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text))
    })
}
use std::{fs, io::{self,Read}, path::Path, process::{Child,Command,Stdio}, os::windows::{process::CommandExt,io::{AsRawHandle,FromRawHandle,OwnedHandle}}, time::{Instant,Duration}};
use windows_sys::Win32::System::JobObjects::*;
fn accepted_sequence(bytes: &[u8], session: &str, last: u64) -> Option<u64> {
    if bytes.len()>16384 || session.is_empty() {return None}
    let v:serde_json::Value=serde_json::from_slice(bytes).ok()?;
    let seq=v["sequence"].as_u64()?;
    if v["version"]!=1 || v["session"].as_str()!=Some(session) || seq<=last || seq>9007199254740991 {return None}
    match v["command"].as_str()? {
        "ping"|"speak"|"cancelSpeech"|"chooseFolder"|"play"|"pause"|"next"|"previous"|"setVolume"|"setShuffle"|"setVoice"=>Some(seq),
        _=>None
    }
}
#[derive(Default)]
pub struct AudioRuntime {session:String, sequence:u64, child:Option<Child>, job:Option<OwnedHandle>, retry:Option<Instant>}
impl AudioRuntime {
    fn stop(&mut self) {
        if let Some(mut child)=self.child.take(){let _=child.kill();let _=child.wait();}
        self.job=None;
    }
    fn start(&mut self, directory:&Path)->io::Result<()> {
        let script=directory.join("planet_audio.ps1");
        fs::write(&script,include_bytes!("../assets/planet_audio.ps1"))?;
        let root=std::env::var_os("SystemRoot").ok_or_else(||io::Error::other("SystemRoot missing"))?;
        let powershell=Path::new(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child=Command::new(powershell).args(["-NoProfile","-NonInteractive","-STA","-ExecutionPolicy","Bypass","-File"])
            .arg(powershell_path(&script)?).arg("-Directory").arg(powershell_path(directory)?).arg("-Session").arg(&self.session)
            .arg("-OwnerPid").arg(std::process::id().to_string()).creation_flags(0x08000000)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(fs::File::create(directory.join("audio-startup.log"))?).spawn()?;
        let job_result=(|| unsafe {
            let handle=CreateJobObjectW(std::ptr::null(),std::ptr::null());
            if handle.is_null(){return Err(io::Error::last_os_error())}
            let job=OwnedHandle::from_raw_handle(handle);
            let mut limits:JOBOBJECT_EXTENDED_LIMIT_INFORMATION=std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(handle,JobObjectExtendedLimitInformation,(&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),std::mem::size_of_val(&limits) as u32)==0
                || AssignProcessToJobObject(handle,child.as_raw_handle())==0 {return Err(io::Error::last_os_error())}
            Ok(job)
        })();
        match job_result {Ok(job)=>{self.job=Some(job);self.child=Some(child);Ok(())},Err(e)=>{let _=child.kill();let _=child.wait();Err(e)}}
    }
    pub fn tick(&mut self,directory:&Path,session:&str) {
        if session.is_empty(){return}
        let read=||->io::Result<Vec<u8>>{let mut b=Vec::new();fs::File::open(directory.join("audio-request.json"))?.take(16385).read_to_end(&mut b)?;Ok(b)};
        if session!=self.session {
            self.stop();self.session=session.into();self.sequence=0;self.retry=None;
            // Discard a previous helper run's mailbox; require a fresh command.
            if let Ok(b)=read(){self.sequence=accepted_sequence(&b,session,0).unwrap_or(0)}
            let _=fs::remove_file(directory.join("audio-worker-request.json"));
            let _=fs::write(directory.join("audio-status.json"),serde_json::json!({"version":1,"session":session,"sequence":self.sequence,"ready":false}).to_string());
            return
        }
        if self.child.as_mut().is_some_and(|p|!matches!(p.try_wait(),Ok(None))) {
            let ack=(||->Option<u64>{
                let mut bytes=Vec::new();
                fs::File::open(directory.join("audio-status.json")).ok()?.take(2*1024*1024).read_to_end(&mut bytes).ok()?;
                let v:serde_json::Value=serde_json::from_slice(&bytes).ok()?;
                if v["session"].as_str()!=Some(session){return None}
                v["sequence"].as_u64()
            })();
            self.sequence=recovery_sequence(self.sequence,ack);
            self.stop();
        }
        let Ok(bytes)=read() else{return};
        let Some(seq)=accepted_sequence(&bytes,session,self.sequence) else{return};
        if self.child.is_none() {
            if self.retry.is_some_and(|t|t.elapsed()<Duration::from_secs(5)){return}
            self.retry=Some(Instant::now());
            if let Err(e)=self.start(directory){
                let _=fs::write(directory.join("audio-status.json"),serde_json::json!({"version":1,"session":session,"sequence":self.sequence,"ready":false,"error":format!("Не удалось запустить звук: {e}")}).to_string());
                return
            }
        }
        if fs::write(directory.join("audio-worker-request.json"),bytes).is_ok(){self.sequence=seq;}
    }
}
impl Drop for AudioRuntime {fn drop(&mut self){self.stop()}}
