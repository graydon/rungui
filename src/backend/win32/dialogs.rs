//! Message box and the `IFileDialog` family (COM, through the `windows` crate's interface wrappers:
//! reference counting and HRESULT -> `Result` are handled by the crate; no vtable slot numbers).

use super::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog,
    IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH,
};
use windows::core::Interface;

pub(super) fn message_box(owner: HWND, spec: &MessageSpec) -> Answer {
    let mut flags = match spec.buttons {
        Buttons::Ok => MESSAGEBOX_STYLE(0),
        Buttons::OkCancel => MB_OKCANCEL,
        Buttons::YesNo => MB_YESNO,
        Buttons::YesNoCancel => MB_YESNOCANCEL,
    };
    flags |= match spec.kind {
        MessageKind::Info => MB_ICONINFORMATION,
        MessageKind::Warning => MB_ICONWARNING,
        MessageKind::Error => MB_ICONERROR,
        MessageKind::Question => MB_ICONQUESTION,
    };
    flags |= MB_APPLMODAL;
    let r = unsafe { MessageBoxW(opt(owner), &hs(&spec.text), &hs(&spec.title), flags) };
    match r {
        IDOK => Answer::Ok,
        IDYES => Answer::Yes,
        IDNO => Answer::No,
        _ => Answer::Cancel,
    }
}

fn item_path(item: &IShellItem) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = Some(String::from_utf16_lossy(p.as_wide()));
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

pub(super) fn file_dialog(owner: HWND, spec: &FileSpec) -> Vec<String> {
    let mut out = vec![];
    let save = spec.mode == FileMode::Save;
    unsafe {
        // IFileOpenDialog is kept for GetResults (multi-select); both derive from IFileDialog
        let open = if save {
            None
        } else {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()
        };
        let dlg: Option<IFileDialog> = if save {
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .ok()
                .and_then(|d| d.cast().ok())
        } else {
            open.as_ref().and_then(|o| o.cast().ok())
        };
        let Some(dlg) = dlg else { return out };

        let mut opts = dlg.GetOptions().unwrap_or_default();
        opts |= FOS_FORCEFILESYSTEM;
        match spec.mode {
            FileMode::PickFolder => opts |= FOS_PICKFOLDERS,
            FileMode::OpenMany => opts |= FOS_ALLOWMULTISELECT,
            _ => {}
        }
        let _ = dlg.SetOptions(opts);
        if !spec.title.is_empty() {
            let _ = dlg.SetTitle(&hs(&spec.title));
        }
        if spec.mode != FileMode::PickFolder && !spec.filters.is_empty() {
            let names: Vec<HSTRING> = spec.filters.iter().map(|(n, _)| hs(n)).collect();
            let pats: Vec<HSTRING> = spec
                .filters
                .iter()
                .map(|(_, e)| {
                    if e.is_empty() {
                        hs("*.*")
                    } else {
                        hs(&e
                            .iter()
                            .map(|x| {
                                format!("*.{}", x.trim_start_matches("*.").trim_start_matches('.'))
                            })
                            .collect::<Vec<_>>()
                            .join(";"))
                    }
                })
                .collect();
            let specs: Vec<COMDLG_FILTERSPEC> = names
                .iter()
                .zip(&pats)
                .map(|(n, p)| COMDLG_FILTERSPEC {
                    pszName: PCWSTR(n.as_ptr()),
                    pszSpec: PCWSTR(p.as_ptr()),
                })
                .collect();
            let _ = dlg.SetFileTypes(&specs);
        }
        if let Some(dir) = &spec.initial_dir {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&hs(dir), None) {
                let _ = dlg.SetFolder(&item);
            }
        }
        if let Some(name) = &spec.initial_name {
            let _ = dlg.SetFileName(&hs(name));
        }
        if dlg.Show(opt(owner)).is_ok() {
            match (&open, spec.mode == FileMode::OpenMany) {
                (Some(open), true) => {
                    if let Ok(arr) = open.GetResults() {
                        let n = arr.GetCount().unwrap_or(0);
                        for i in 0..n {
                            if let Ok(item) = arr.GetItemAt(i) {
                                out.extend(item_path(&item));
                            }
                        }
                    }
                }
                _ => {
                    if let Ok(item) = dlg.GetResult() {
                        out.extend(item_path(&item));
                    }
                }
            }
        }
    }
    out
}
