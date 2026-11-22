//! Images in the conversation: a PNG pasted into the composer, and the ones user messages carry
//! (saved with the history as base64). Decoding and shrinking them runs on a thread of its own,
//! never in `view` or `poll`; what it makes lands in [`KrampstaApp::pump`], in the session's
//! `SessionView`: the draft's attachment, or a message's thumbnail.

use std::sync::{Arc, OnceLock, mpsc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder as _, codecs::png::PngDecoder};
use krampsta_agent::Notifier;
use krampsta_core::{Message, MessageId, RunId, Session, SessionId};
use rune::{ClipboardImage, Pixels, TextEdit};

use crate::state::KrampstaApp;

/// The largest PNG a message may carry.
pub const MAX_PNG_BYTES: usize = 5 * 1024 * 1024;
/// The most pixels it may have.
pub const MAX_PIXELS: u64 = 24_000_000;
/// The box a message's thumbnail fits in, in pixels.
const THUMB: (u32, u32) = (320, 240);

const TOO_LARGE: &str = "The image is too large (5 MiB as PNG and 24 megapixels at most).";
const UNREADABLE: &str = "The clipboard image couldn't be read.";

/// An image attached to the unsent draft: the PNG it sends, and its thumbnail.
#[derive(Clone)]
pub struct DraftImage {
    pub png: Arc<str>,
    pub thumb: Pixels,
}

impl std::fmt::Debug for DraftImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftImage").field("base64_bytes", &self.png.len()).field("thumb", &self.thumb).finish()
    }
}

/// A message's image, as the transcript shows it.
#[derive(Clone, Debug)]
pub enum MessageImage {
    Loading,
    Ready(Pixels),
    Unreadable,
}

/// The draft sent with an image, kept until its run ends: if it fails, both go back to the
/// composer to try again.
#[derive(Debug)]
pub(crate) struct SentImage {
    pub run: RunId,
    pub text: String,
    pub image: DraftImage,
}

enum Job {
    Draft { session: SessionId, image: ClipboardImage },
    Message { session: SessionId, message: MessageId, png: Arc<str> },
}

enum Done {
    Draft { session: SessionId, image: Result<DraftImage, &'static str> },
    Message { session: SessionId, message: MessageId, thumb: Option<Pixels> },
}

/// The decoding thread, started by the first image.
#[derive(Default)]
pub(crate) struct Images {
    worker: Option<Worker>,
    notify: Arc<OnceLock<Notifier>>,
}

struct Worker {
    // Unbounded: the UI thread must never wait to queue, and each image is queued once
    // (a paste, or a message as its session lands).
    jobs: mpsc::Sender<Job>,
    done: mpsc::Receiver<Done>,
}

impl Images {
    pub fn set_notifier(&self, notify: Notifier) {
        let _ = self.notify.set(notify);
    }

    fn queue(&mut self, job: Job) -> Result<(), &'static str> {
        let worker = match &mut self.worker {
            Some(worker) => worker,
            None => self.worker.insert(spawn(self.notify.clone()).map_err(|_| "Images can't be read right now.")?),
        };
        worker.jobs.send(job).map_err(|_| "Images can't be read right now.")
    }

    fn take_done(&mut self) -> Option<Done> {
        self.worker.as_ref()?.done.try_recv().ok()
    }
}

fn spawn(notify: Arc<OnceLock<Notifier>>) -> std::io::Result<Worker> {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (outbox, done) = mpsc::channel::<Done>();
    std::thread::Builder::new().name("krampsta-images".into()).spawn(move || {
        for job in inbox {
            let result = match job {
                Job::Draft { session, image } => Done::Draft { session, image: draft(&image) },
                Job::Message { session, message, png } => Done::Message { session, message, thumb: thumbnail(&png) },
            };
            if outbox.send(result).is_err() {
                return;
            }
            if let Some(notify) = notify.get() {
                notify();
            }
        }
    })?;
    Ok(Worker { jobs, done })
}

/// Checks a pasted image's size from its header, before anything decodes it.
fn check_size(image: &ClipboardImage) -> Result<(), &'static str> {
    let (width, height) = image.size();
    if image.png().len() > MAX_PNG_BYTES || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(TOO_LARGE);
    }
    Ok(())
}

fn draft(image: &ClipboardImage) -> Result<DraftImage, &'static str> {
    check_size(image)?;
    let thumb = decode(image.png()).ok_or(UNREADABLE)?;
    Ok(DraftImage { png: STANDARD.encode(image.png()).into(), thumb })
}

/// A saved message's PNG (base64) as a thumbnail, within the same limits as a paste.
pub fn thumbnail(png: &str) -> Option<Pixels> {
    if png.len() > MAX_PNG_BYTES.div_ceil(3) * 4 {
        return None;
    }
    decode(&STANDARD.decode(png).ok()?)
}

/// Decodes a PNG and shrinks it to fit [`THUMB`], checking its size before allocating pixels.
fn decode(png: &[u8]) -> Option<Pixels> {
    if png.len() > MAX_PNG_BYTES {
        return None;
    }
    let decoder = PngDecoder::new(std::io::Cursor::new(png)).ok()?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let image = DynamicImage::from_decoder(decoder).ok()?;
    let fits = width <= THUMB.0 && height <= THUMB.1;
    let thumb = if fits { image } else { image.thumbnail(THUMB.0, THUMB.1) }.into_rgba8();
    let pixels = Pixels::new();
    pixels.set(thumb.width(), thumb.height(), thumb.into_raw());
    Some(pixels)
}

impl KrampstaApp {
    /// Lets the image thread wake the window when an image is ready.
    pub(crate) fn set_images_notifier(&self, notify: Notifier) {
        self.images.set_notifier(notify);
    }

    /// Attaches a pasted image to `session`'s draft once it is read, in the background.
    pub(crate) fn attach_image(&mut self, session: SessionId, image: ClipboardImage) {
        if let Err(e) = check_size(&image) {
            self.notice = Some(e.into());
            return;
        }
        match self.images.queue(Job::Draft { session, image }) {
            Ok(()) => {
                self.view_mut(session).attaching = true;
                self.notice = None;
            }
            Err(e) => self.notice = Some(e.into()),
        }
    }

    pub(crate) fn remove_attachment(&mut self, session: SessionId) {
        let view = self.view_mut(session);
        view.attachment = None;
        view.attaching = false;
        self.notice = None;
    }

    /// Whether `session`'s provider can read images: only then is an attachment sent.
    pub(crate) fn check_attachment(&self, session: SessionId) -> Result<Option<Arc<str>>, String> {
        let Some(view) = self.views.get(&session) else { return Ok(None) };
        if view.attaching {
            return Err("The image is still being read.".into());
        }
        let Some(image) = &view.attachment else { return Ok(None) };
        if !self.engine.takes_images() {
            let provider = self.engine.provider_label();
            return Err(format!("{provider} can't read images. Choose one that can, or remove the image."));
        }
        Ok(Some(image.png.clone()))
    }

    /// After a send with an image: the message shows the draft's thumbnail, and the draft is
    /// kept until the run ends.
    pub(crate) fn sent_attachment(&mut self, session: SessionId, run: RunId, message: MessageId, text: String) {
        let view = self.view_mut(session);
        let Some(image) = view.attachment.take() else { return };
        view.images.insert(message, MessageImage::Ready(image.thumb.clone()));
        view.sent = Some(SentImage { run, text, image });
    }

    /// When `run` ends: a failed one gives its text and image back to an empty composer.
    pub(crate) fn finish_image_send(&mut self, session: SessionId, run: RunId, failed: bool) {
        let Some(view) = self.views.get_mut(&session) else { return };
        if !view.sent.as_ref().is_some_and(|sent| sent.run == run) {
            return;
        }
        let Some(sent) = view.sent.take() else { return };
        if failed && view.attachment.is_none() && !view.attaching && view.draft.text.is_empty() {
            let focused = view.draft.focused;
            view.draft = TextEdit::new(&sent.text);
            view.draft.focused = focused;
            view.attachment = Some(sent.image);
        }
    }

    /// Starts reading the images of sessions that just arrived (from the history).
    pub(crate) fn read_images(&mut self, session: &Session) {
        for m in session.messages.iter().filter(|m| m.image_png.is_some()) {
            self.read_image(session.id, m);
        }
    }

    fn read_image(&mut self, session: SessionId, m: &Message) {
        let Some(png) = m.image_png.clone() else { return };
        let state = match self.images.queue(Job::Message { session, message: m.id, png }) {
            Ok(()) => MessageImage::Loading,
            Err(_) => MessageImage::Unreadable,
        };
        self.view_mut(session).images.insert(m.id, state);
    }

    /// Lands what the image thread finished. True if anything did.
    pub(crate) fn land_images(&mut self) -> bool {
        let mut changed = false;
        while let Some(done) = self.images.take_done() {
            changed = true;
            match done {
                Done::Draft { session, image } => {
                    let view = self.view_mut(session);
                    // Removed while it was read: drop it.
                    if !std::mem::take(&mut view.attaching) {
                        continue;
                    }
                    match image {
                        Ok(image) => view.attachment = Some(image),
                        Err(e) => self.notice = Some(e.into()),
                    }
                }
                Done::Message { session, message, thumb } => {
                    if let Some(view) = self.views.get_mut(&session) {
                        let state = thumb.map_or(MessageImage::Unreadable, MessageImage::Ready);
                        view.images.insert(message, state);
                    }
                }
            }
        }
        changed
    }

    /// A message's image in the transcript.
    pub fn message_image(&self, session: SessionId, message: MessageId) -> Option<&MessageImage> {
        self.views.get(&session)?.images.get(&message)
    }

    /// Shows a message's image at once, decoded here (scenes).
    fn show_image_now(&mut self, session: SessionId, message: MessageId, png: &str) {
        let state = thumbnail(png).map_or(MessageImage::Unreadable, MessageImage::Ready);
        self.view_mut(session).images.insert(message, state);
    }
}

/// The `image` scene: a conversation that began with a screenshot, and another one pasted
/// into the draft.
pub(crate) fn scene(s: &mut KrampstaApp) {
    use krampsta_core::AgentEvent;

    let Some(png) = sample_png() else { return };
    let encoded: Arc<str> = STANDARD.encode(&png).into();
    let session = s.new_session();
    let run = RunId(10_001);
    let Some(x) = s.sessions.iter_mut().find(|x| x.id == session) else { return };
    let Ok(message) = x.submit_with_image("Why is the last button cut off here?", Some(encoded.clone()), run) else {
        return;
    };
    let reply = "The toolbar row has a fixed width, so its last button overflows. Let it shrink.";
    for event in
        [AgentEvent::Started { run }, AgentEvent::TextDelta { run, text: reply.into() }, AgentEvent::Completed { run }]
    {
        x.apply(&event);
    }
    s.show_image_now(session, message, &encoded);
    if let Some(image) = ClipboardImage::from_png(png).and_then(|i| draft(&i).ok()) {
        s.view_mut(session).attachment = Some(image);
    }
}

/// A made-up screenshot: a window with a toolbar of buttons, the last one cut off.
fn sample_png() -> Option<Vec<u8>> {
    let (width, height) = (480, 300);
    let mut img = image::RgbaImage::from_pixel(width, height, image::Rgba([24, 26, 32, 255]));
    let mut fill = |x0: u32, y0: u32, w: u32, h: u32, c: [u8; 4]| {
        for y in y0..(y0 + h).min(height) {
            for x in x0..(x0 + w).min(width) {
                img.put_pixel(x, y, image::Rgba(c));
            }
        }
    };
    fill(0, 0, width, 36, [38, 41, 50, 255]);
    for i in 0..5 {
        fill(16 + i * 100, 8, 84, 20, [72, 110, 220, 255]);
    }
    fill(16, 60, 300, 12, [70, 74, 86, 255]);
    fill(16, 84, 420, 12, [58, 62, 72, 255]);
    fill(16, 108, 360, 12, [58, 62, 72, 255]);
    fill(16, 150, 448, 130, [32, 35, 43, 255]);
    let mut out = Vec::new();

    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(width, height, image::Rgba([200, 40, 160, 255]))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn a_large_image_shrinks_to_fit_its_thumbnail_and_a_small_one_keeps_its_size() {
        let big = thumbnail(&STANDARD.encode(png(1000, 500))).unwrap();
        assert_eq!(big.size(), (320, 160));
        let small = thumbnail(&STANDARD.encode(png(4, 3))).unwrap();
        assert_eq!(small.size(), (4, 3));
    }

    #[test]
    fn what_isnt_a_png_is_unreadable() {
        assert!(thumbnail("not base64!").is_none());
        assert!(thumbnail(&STANDARD.encode(b"\x89PNG but not really")).is_none());
    }

    #[test]
    fn an_image_is_only_sent_to_a_provider_that_reads_images() {
        use krampsta_agent::{MockCatalog, MockConfig};

        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
        let session = app.selected;
        let image = draft(&ClipboardImage::from_png(png(8, 8)).unwrap()).unwrap();
        app.view_mut(session).attachment = Some(image);
        for p in &mut app.engine.providers {
            p.images = false;
        }
        let refused = app.send().unwrap_err().to_string();
        assert!(refused.contains("can't read images"), "{refused}");
        assert!(app.session(session).unwrap().messages.is_empty(), "nothing was sent");
        assert!(app.views[&session].attachment.is_some(), "the image stays in the draft");
    }

    #[test]
    fn a_pasted_image_is_checked_by_its_header_before_decoding() {
        // Only the header: 30 megapixels never get allocated.
        let header = [&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..], &6000u32.to_be_bytes(), &5000u32.to_be_bytes()].concat();
        let image = ClipboardImage::from_png(header).unwrap();
        assert_eq!(check_size(&image), Err(TOO_LARGE));
        let image = ClipboardImage::from_png(png(8, 8)).unwrap();
        let draft = draft(&image).unwrap();
        assert_eq!(STANDARD.decode(draft.png.as_bytes()).unwrap(), image.png());
    }
}
