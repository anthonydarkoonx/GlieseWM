//! The requests on the display endpoint (`gliese_proto::op`).
use crate::{Buf, Host, Rect, Server, Session, Task, WinId, Window};
use alloc::string::String;
use gliese_proto::{self as p, ev, op, session as S, Event};

fn err(e: i64) -> [u64; 3] {
    [e as u64, 0, 0]
}
const OK: [u64; 3] = [0, 0, 0];

impl Server {
    /// One call on the display endpoint from task `sender`; the reply to send back.
    pub fn request(&mut self, host: &mut dyn Host, sender: Task, w: [u64; 3]) -> [u64; 3] {
        let r = self.request_inner(host, sender, w);
        self.st.flush_wakes(host);
        r
    }

    fn session_of(&self, host: &dyn Host, sender: Task) -> Option<usize> {
        let program = host.program_of(sender);
        self.st.sessions.iter().position(|s| s.as_ref().is_some_and(|s| s.program == program && s.tasks.contains(&sender)))
    }

    fn window_of(&self, sess: usize, num: u8) -> Option<WinId> {
        self.st.windows.iter().find(|w| w.sess == sess && w.num == num).map(|w| w.id)
    }

    fn request_inner(&mut self, host: &mut dyn Host, sender: Task, w: [u64; 3]) -> [u64; 3] {
        let (opc, num, arg) = p::split_w0(w[0]);
        if (op::MGR_FIRST..=op::MGR_LAST).contains(&opc) {
            return self.manager_request(host, sender, opc, arg, w);
        }
        if opc == op::HELLO {
            return self.hello(host, sender, arg);
        }
        let Some(sess) = self.session_of(host, sender) else { return err(p::E_PERM) };
        match opc {
            op::BYE => {
                self.end_session(host, sess);
                OK
            }
            op::CREATE => self.create(host, sess, num, w[1], w[2]),
            op::GL_RELOAD => {
                // anyone may ask: it only makes the desktop role read the user's own file again
                self.st.agent_queue.push(crate::AgentRequest::Reload);
                self.deliver_agent(host);
                OK
            }
            op::GL_QUERY => err(p::E_INVAL),
            op::GL_CONFIG_BEGIN | op::GL_CONFIG_CHUNK | op::GL_CONFIG_END | op::GL_NEW_WINDOW | op::GL_PROGRAM_NAME => {
                if self.st.sessions[sess].as_ref().map(|s| s.role) != Some(p::role::DESKTOP) {
                    return err(p::E_PERM);
                }
                self.desktop_request(host, sess, opc, arg, w)
            }
            _ => {
                let Some(id) = self.window_of(sess, num) else { return err(p::E_BADWIN) };
                self.window_request(host, sess, sender, id, opc, arg, w)
            }
        }
    }

    /// The desktop role's requests (`gliesed`): the config upload, new windows for running programs, program ids.
    fn desktop_request(&mut self, host: &mut dyn Host, sess: usize, opc: u8, arg: u32, w: [u64; 3]) -> [u64; 3] {
        let page = self.st.sessions[sess].as_ref().map(|s| s.page);
        let mut buf = [0u8; S::SCRATCH_BYTES];
        let scratch = |host: &mut dyn Host, n: usize, buf: &mut [u8; S::SCRATCH_BYTES]| -> Option<alloc::vec::Vec<u8>> {
            let pg = host.shm(page?)?;
            Some(p::scratch_read(pg, n, buf).to_vec())
        };
        match opc {
            op::GL_CONFIG_BEGIN => {
                if arg as usize > p::MAX_CONFIG_BYTES {
                    return err(p::E_INVAL);
                }
                if let Some(s) = self.st.sessions[sess].as_mut() {
                    s.upload.clear();
                }
                OK
            }
            op::GL_CONFIG_CHUNK => {
                let Some(bytes) = scratch(host, arg as usize, &mut buf) else { return err(p::E_NOMEM) };
                let s = self.st.sessions[sess].as_mut().expect("there");
                if s.upload.len() + bytes.len() > p::MAX_CONFIG_BYTES {
                    return err(p::E_INVAL);
                }
                s.upload.extend_from_slice(&bytes);
                OK
            }
            op::GL_CONFIG_END => {
                let bytes = core::mem::take(&mut self.st.sessions[sess].as_mut().expect("there").upload);
                let text = String::from_utf8_lossy(&bytes);
                let r = self.with_policy(host, |pol, c| pol.config_text(c, "gliese.conf", &text));
                match r {
                    Ok(()) => OK,
                    Err((line, col, msg)) => {
                        let n = page.and_then(|pg| host.shm(pg)).map_or(0, |pg| p::scratch_write(pg, msg.as_bytes()));
                        [p::E_CONFIG as u64, line as u64, col as u64 | (n as u64) << 32]
                    }
                }
            }
            op::GL_NEW_WINDOW | op::GL_PROGRAM_NAME => {
                let program = host.program_of(w[1] as Task);
                let Some(target) = self.st.sessions.iter().position(|s| s.as_ref().is_some_and(|s| s.program == program)) else {
                    return if opc == op::GL_NEW_WINDOW { err(p::E_BADWIN) } else { self.name(host, program, arg, &mut buf, page) };
                };
                if opc == op::GL_NEW_WINDOW {
                    self.st.send(host, target, Event { kind: ev::NEW_WINDOW, ..Default::default() });
                    OK
                } else {
                    self.name(host, program, arg, &mut buf, page)
                }
            }
            _ => err(p::E_INVAL),
        }
    }

    fn name(&mut self, host: &mut dyn Host, program: crate::Program, n: u32, buf: &mut [u8; S::SCRATCH_BYTES], page: Option<crate::ShmId>) -> [u64; 3] {
        let Some(pg) = page.and_then(|pg| host.shm(pg)) else { return err(p::E_NOMEM) };
        let name = String::from_utf8_lossy(p::scratch_read(pg, (n as usize).min(16), buf)).into_owned();
        self.with_policy(host, |pol, _| pol.program_named(program, &name));
        OK
    }

    fn manager_request(&mut self, host: &mut dyn Host, sender: Task, opc: u8, arg: u32, w: [u64; 3]) -> [u64; 3] {
        if self.st.manager != Some(sender) {
            return err(p::E_PERM);
        }
        match opc {
            op::MGR_SET_ROLE => {
                let program = host.program_of(w[1] as Task);
                if arg == p::role::DESKTOP {
                    self.st.desktop_program = Some(program);
                }
                for s in self.st.sessions.iter_mut().flatten().filter(|s| s.program == program) {
                    s.role = arg;
                }
                OK
            }
            _ => err(p::E_INVAL),
        }
    }

    fn hello(&mut self, host: &mut dyn Host, sender: Task, version: u32) -> [u64; 3] {
        if version != p::VERSION {
            return err(p::E_INVAL);
        }
        let program = host.program_of(sender);
        let existing = self.st.sessions.iter().position(|s| s.as_ref().is_some_and(|s| s.program == program));
        let i = match existing {
            Some(i) => i,
            None => {
                if self.st.sessions.iter().flatten().count() >= self.st.limits.sessions {
                    return err(p::E_FULL);
                }
                let page = match host.shm_create(S::PAGES) {
                    Ok(p) => p,
                    Err(e) => return err(e),
                };
                let (sw, sh) = self.st.screen;
                if let Some(pg) = host.shm(page) {
                    pg[S::W_MAGIC] = S::MAGIC;
                    pg[S::W_VERSION] = p::VERSION;
                    pg[S::W_SCREEN_W] = sw as u32;
                    pg[S::W_SCREEN_H] = sh as u32;
                }
                let role = if self.st.desktop_program == Some(program) { p::role::DESKTOP } else { p::role::NONE };
                let s = Session { program, tasks: alloc::vec::Vec::new(), page, role, wake: false, pools: alloc::vec::Vec::new(), upload: alloc::vec::Vec::new() };
                match self.st.sessions.iter().position(|s| s.is_none()) {
                    Some(i) => {
                        self.st.sessions[i] = Some(s);
                        i
                    }
                    None => {
                        self.st.sessions.push(Some(s));
                        self.st.sessions.len() - 1
                    }
                }
            }
        };
        let page = self.st.sessions[i].as_ref().expect("just made").page;
        let slot = match host.shm_grant(page, sender) {
            Ok(s) => s,
            Err(e) => {
                if existing.is_none() {
                    self.end_session(host, i);
                }
                return err(e);
            }
        };
        let s = self.st.sessions[i].as_mut().expect("there");
        if !s.tasks.contains(&sender) {
            s.tasks.push(sender);
        }
        // the desktop role connecting: what waited for it
        if s.role == p::role::DESKTOP {
            self.deliver_agent(host);
            self.st.flush_wakes(host);
        }
        [0, slot as u64, 0]
    }

    fn create(&mut self, host: &mut dyn Host, sess: usize, num: u8, size: u64, parent: u64) -> [u64; 3] {
        let (w, h) = p::split_size(size);
        if num as usize >= S::MAX_WINDOWS || w == 0 || h == 0 || w > p::MAX_SIDE || h > p::MAX_SIDE {
            return err(p::E_INVAL);
        }
        if self.window_of(sess, num).is_some() {
            return err(p::E_INVAL);
        }
        if self.st.windows.len() >= self.st.limits.windows_total {
            return err(p::E_FULL);
        }
        let parent = match parent {
            0 => None,
            n => match self.window_of(sess, (n - 1) as u8) {
                Some(p) => Some(p),
                None => return err(p::E_BADWIN),
            },
        };
        let id = self.st.next_id;
        self.st.next_id += 1;
        self.st.windows.push(Window {
            id,
            sess,
            num,
            parent,
            title: String::new(),
            outer: Rect::default(),
            border: 0,
            requested: (w, h),
            shown: None,
            front: 0,
            next: None,
            conf: None,
            mapped: false,
            hidden: false,
            min: (0, 0),
            max: (0, 0),
            state: 0,
            cursor: p::cursor::ARROW,
            frame_wanted: false,
        });
        self.with_policy(host, |pol, c| pol.created(c, id));
        // a window the policy did not place gets what it asked for, centred
        if self.st.win(id).is_some_and(|w| w.conf.is_none()) {
            let (sw, sh) = self.st.screen;
            let r = Rect::new((sw - w as i32) / 2, (sh - h as i32) / 2, w as i32, h as i32);
            self.with_policy(host, |_, c| c.place(id, r, 0, 0));
        }
        OK
    }

    #[allow(clippy::too_many_arguments)]
    fn window_request(&mut self, host: &mut dyn Host, sess: usize, sender: Task, id: WinId, opc: u8, arg: u32, w: [u64; 3]) -> [u64; 3] {
        match opc {
            op::DESTROY => {
                self.destroy_window(host, id);
                OK
            }
            op::SET_TITLE => {
                let page = self.st.sessions[sess].as_ref().map(|s| s.page);
                let mut buf = [0u8; S::SCRATCH_BYTES];
                let title = match page.and_then(|pg| host.shm(pg)) {
                    Some(pg) => String::from_utf8_lossy(p::scratch_read(pg, arg as usize, &mut buf)).chars().take(256).collect(),
                    None => return err(p::E_NOMEM),
                };
                if let Some(win) = self.st.win_mut(id) {
                    win.title = title;
                }
                self.with_policy(host, |pol, c| pol.title_changed(c, id));
                OK
            }
            op::SET_LIMITS => {
                let (min, max) = (p::split_size(w[1]), p::split_size(w[2]));
                if (max.0 > 0 && max.0 < min.0) || (max.1 > 0 && max.1 < min.1) {
                    return err(p::E_INVAL);
                }
                if let Some(win) = self.st.win_mut(id) {
                    win.min = min;
                    win.max = max;
                }
                OK
            }
            op::ACK_CONFIGURE => self.ack(host, sess, id, arg, sender),
            op::COMMIT => self.commit(host, id, arg, w[1], w[2] as u32),
            op::SET_CURSOR => {
                if let Some(win) = self.st.win_mut(id) {
                    win.cursor = arg;
                }
                OK
            }
            op::WANT => {
                self.with_policy(host, |pol, c| pol.wants(c, id, arg));
                OK
            }
            _ => err(p::E_INVAL),
        }
    }

    /// The client is ready for the configure `serial`: hand it buffers of that size (new ones if the size changed), in one of its
    /// program's pools, and the slot `sender` has that pool in.
    fn ack(&mut self, host: &mut dyn Host, sess: usize, id: WinId, serial: u32, sender: Task) -> [u64; 3] {
        let Some(win) = self.st.win(id) else { return err(p::E_BADWIN) };
        let Some(conf) = win.conf.filter(|c| c.serial == serial) else { return err(p::E_AGAIN) };
        let have = win.next.or(win.shown);
        let buf = match have.filter(|b| (b.w, b.h) == (conf.w, conf.h)) {
            Some(b) => b,
            None => {
                // new buffers; ones handed out before and never committed go now
                if let Some(old) = self.st.win_mut(id).and_then(|w| w.next.take()) {
                    self.st.free_buf(host, sess, old.pool, old.offset, old.pages());
                }
                let (pool, offset) = match self.st.alloc_buf(host, sess, p::buffer_pages(conf.w, conf.h)) {
                    Ok(x) => x,
                    Err(e) => return err(e),
                };
                let generation = have.map_or(1, |b| b.generation.wrapping_add(1).max(1));
                let b = Buf { pool, offset, w: conf.w, h: conf.h, generation };
                self.st.win_mut(id).expect("still there").next = Some(b);
                self.st.write_table(host, id);
                b
            }
        };
        match self.st.pool_slot(host, sess, buf.pool, sender) {
            Ok(slot) => [0, slot as u64 | (buf.pool as u64) << 32, buf.offset as u64 | (buf.generation as u64) << 32],
            Err(e) => err(e),
        }
    }

    fn commit(&mut self, host: &mut dyn Host, id: WinId, arg: u32, damage: u64, generation: u32) -> [u64; 3] {
        let index = (arg & 0xFF) as u8;
        if index > 1 {
            return err(p::E_INVAL);
        }
        let Some(win) = self.st.win_mut(id) else { return err(p::E_BADWIN) };
        let mut retired = None;
        match (win.next, win.shown) {
            (Some(n), _) if n.generation == generation => {
                retired = win.shown.take();
                win.shown = Some(n);
                win.next = None;
            }
            (_, Some(s)) if s.generation == generation => {}
            _ => return err(p::E_AGAIN),
        }
        win.front = index;
        let first = !win.mapped;
        win.mapped = true;
        if arg & p::COMMIT_FRAME != 0 {
            win.frame_wanted = true;
        }
        let client = win.client_rect();
        let visible = win.visible();
        let wanted_frame = win.frame_wanted;
        if let Some(old) = retired {
            let sess = self.st.win(id).map_or(0, |w| w.sess);
            self.st.free_buf(host, sess, old.pool, old.offset, old.pages());
        }
        if visible {
            if first {
                let o = self.st.win(id).map(|w| w.outer).unwrap_or_default();
                self.st.damage.add(o);
            } else if damage == 0 || retired.is_some() {
                self.st.damage.add(client);
            } else {
                let (x, y, dw, dh) = p::split_rect(damage);
                let r = Rect::new(client.x + x as i32, client.y + y as i32, dw as i32, dh as i32);
                self.st.damage.add(crate::intersect(&r, &client));
            }
        }
        if wanted_frame && !self.st.frames.contains(&id) {
            self.st.frames.push(id);
        }
        self.st.write_table(host, id);
        OK
    }

    /// FRAME_DONE to the windows that asked, once their frame is on screen (called by `render`).
    pub(crate) fn frames_done(&mut self, host: &mut dyn Host) {
        let frames = core::mem::take(&mut self.st.frames);
        for id in frames {
            if let Some(w) = self.st.win_mut(id) {
                w.frame_wanted = false;
            }
            self.st.send_to(host, id, Event { kind: ev::FRAME_DONE, ..Default::default() });
        }
    }
}
