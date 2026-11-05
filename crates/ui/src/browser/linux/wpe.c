#define _GNU_SOURCE
// WPE WebKit runs in its own process behind a WPEPlatform implemented here, so
// Zeron is the compositor: WebKit renders dma-bufs on GPUI's GPU, in a layout
// GPUI can import, GPUI copies their damaged regions into a persistent atlas
// tile, and GPUI's own frame callbacks drive WebKit's vblank. Frames GPUI cannot
// import, and those of pages rendered without a GPU, cross through a
// shared-memory mirror instead. Packets use the WebKitGTK helper's framing over
// a Unix socket, which also carries the buffers' file descriptors.
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <errno.h>
#include <fcntl.h>
#include <gbm.h>
#include <glib-unix.h>
#include <json-glib/json-glib.h>
#include <math.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>
#include <wpe/webkit.h>
#include <wpe/wpe-platform.h>
#include <xkbcommon/xkbcommon.h>

// Largest packet either side takes; larger ones end the connection.
#define MAX_PACKET (64 * 1024 * 1024)
#define MAX_DIMENSION 8192
#define FOURCC_ARGB8888 0x34325241u // 'AR24', GPUI's BGRA atlas byte order

typedef struct _Page Page;
typedef struct _ZSync ZSync;
static GHashTable *pages;
static GByteArray *input;
static GMainLoop *loop;
static WPEDisplay *display;
static WebKitNetworkSession *session;
static WebKitSettings *settings;
static int proto = -1, profile_lock = -1;
static const char *render_node;
static GArray *importable; // guint64 AR24 modifiers GPUI imports
static GMutex proto_lock;
static GAsyncQueue *ticks;
static guint next_buffer = 1;
static gboolean shutting_down;
// The page whose WebKitWebView is being constructed: WebKit creates its view
// and toplevel through the display before the constructor returns.
static Page *creating;

struct _Page {
    guint id;
    WebKitWebView *web;
    WPEView *view;
    WPEScreen *screen;
    gboolean visible;
    gchar *error;
    int width, height; // device pixels, as Zeron lays the page out
    double scale;
    WebKitOptionMenu *options;
    gchar *context_link;
    double pointer_x, pointer_y; // logical pixels
    guint buttons;               // pressed buttons as WPE modifiers
    WPEBuffer *rendered;         // shown to Zeron, waits for "presented"
    GHashTable *held;            // buffer id -> WPEBuffer Zeron may still read
    gboolean readback;           // deliver GPU frames through the mirror
    int mirror_fd;
    guint8 *mirror;
    guint mirror_id;
    int mirror_width, mirror_height;
};

// ---------------------------------------------------------------- protocol
static void send_packet_fd(char kind, guint id, const void *data, gsize length, int fd) {
    // Zeron would drop the connection, and every page with it, over one
    // page's huge title or copy.
    if (length > MAX_PACKET) {
        g_printerr("zeron-wpe: dropping a '%c' packet of %zu bytes\n", kind, length);
        return;
    }
    guint8 header[9] = {(guint8)kind};
    guint32 fields[2] = {GUINT32_TO_LE(id), GUINT32_TO_LE((guint32)length)};
    memcpy(header + 1, fields, sizeof fields);
    struct iovec iov[2] = {{header, sizeof header}, {(void *)data, length}};
    union {
        char buffer[CMSG_SPACE(sizeof(int))];
        struct cmsghdr align;
    } control;
    struct msghdr message = {.msg_iov = iov, .msg_iovlen = length ? 2 : 1};
    if (fd >= 0) {
        message.msg_control = control.buffer;
        message.msg_controllen = sizeof control.buffer;
        struct cmsghdr *c = CMSG_FIRSTHDR(&message);
        c->cmsg_level = SOL_SOCKET;
        c->cmsg_type = SCM_RIGHTS;
        c->cmsg_len = CMSG_LEN(sizeof(int));
        memcpy(CMSG_DATA(c), &fd, sizeof(int));
    }
    gboolean broken = FALSE;
    g_mutex_lock(&proto_lock);
    while (message.msg_iovlen && !broken) {
        ssize_t n = sendmsg(proto, &message, MSG_NOSIGNAL);
        if (n < 0 && errno == EINTR)
            continue;
        if (n <= 0) {
            broken = TRUE;
            break;
        }
        // The descriptor travels with the first byte only.
        message.msg_control = NULL;
        message.msg_controllen = 0;
        while (n > 0 && message.msg_iovlen) {
            if ((size_t)n >= message.msg_iov->iov_len) {
                n -= message.msg_iov->iov_len;
                message.msg_iov++;
                message.msg_iovlen--;
            } else {
                message.msg_iov->iov_base = (char *)message.msg_iov->iov_base + n;
                message.msg_iov->iov_len -= n;
                n = 0;
            }
        }
    }
    g_mutex_unlock(&proto_lock);
    if (broken)
        g_main_loop_quit(loop);
}
static void send_packet(char kind, guint id, const void *data, gsize length) {
    send_packet_fd(kind, id, data, length, -1);
}
static void send_json_fd(char kind, guint id, JsonBuilder *builder, int fd) {
    JsonGenerator *gen = json_generator_new();
    JsonNode *root = json_builder_get_root(builder);
    json_generator_set_root(gen, root);
    gsize length;
    gchar *data = json_generator_to_data(gen, &length);
    send_packet_fd(kind, id, data, length, fd);
    g_free(data);
    json_node_free(root);
    g_object_unref(gen);
    g_object_unref(builder);
}
static void send_json(char kind, guint id, JsonBuilder *builder) { send_json_fd(kind, id, builder, -1); }
static void member_string(JsonBuilder *b, const char *name, const char *value) {
    json_builder_set_member_name(b, name);
    if (value)
        json_builder_add_string_value(b, value);
    else
        json_builder_add_null_value(b);
}
static void member_bool(JsonBuilder *b, const char *name, gboolean value) {
    json_builder_set_member_name(b, name);
    json_builder_add_boolean_value(b, value);
}
static void member_int(JsonBuilder *b, const char *name, gint64 value) {
    json_builder_set_member_name(b, name);
    json_builder_add_int_value(b, value);
}

// ---------------------------------------------------------------- input method
// Relay the platform IME through GPUI while WebKit retains editor semantics.
typedef struct {
    WebKitInputMethodContext parent;
    guint id;
    gchar *preedit;
    guint cursor;
} BrowserIM;
typedef struct {
    WebKitInputMethodContextClass parent;
} BrowserIMClass;
G_DEFINE_TYPE(BrowserIM, browser_im, WEBKIT_TYPE_INPUT_METHOD_CONTEXT)
static void im_preedit(WebKitInputMethodContext *ctx, gchar **text, GList **underlines,
                       guint *cursor) {
    BrowserIM *im = (BrowserIM *)ctx;
    *text = g_strdup(im->preedit ?: "");
    *cursor = im->cursor;
    *underlines =
        im->preedit && *im->preedit
            ? g_list_append(NULL,
                            webkit_input_method_underline_new(0, g_utf8_strlen(im->preedit, -1)))
            : NULL;
}
static gboolean im_filter(WebKitInputMethodContext *ctx, gpointer event) { return FALSE; }
static void im_focus(WebKitInputMethodContext *ctx, gboolean focused) {
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    member_bool(b, "focused", focused);
    json_builder_end_object(b);
    send_json('I', ((BrowserIM *)ctx)->id, b);
}
static void im_focus_in(WebKitInputMethodContext *ctx) { im_focus(ctx, TRUE); }
static void im_focus_out(WebKitInputMethodContext *ctx) { im_focus(ctx, FALSE); }
static void im_cursor(WebKitInputMethodContext *ctx, int x, int y, int w, int h) {
    // WPE reports logical pixels; Zeron's input handler expects device pixels.
    Page *p = g_hash_table_lookup(pages, GUINT_TO_POINTER(((BrowserIM *)ctx)->id));
    double scale = p ? p->scale : 1;
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    json_builder_set_member_name(b, "caret");
    json_builder_begin_array(b);
    json_builder_add_int_value(b, lround(x * scale));
    json_builder_add_int_value(b, lround(y * scale));
    json_builder_add_int_value(b, lround(w * scale));
    json_builder_add_int_value(b, lround(h * scale));
    json_builder_end_array(b);
    json_builder_end_object(b);
    send_json('I', ((BrowserIM *)ctx)->id, b);
}
static void im_surrounding(WebKitInputMethodContext *ctx, const gchar *text, guint length,
                           guint cursor, guint selection) {
    // Password contents must never leave WebKit's editor process boundary.
    if (webkit_input_method_context_get_input_purpose(ctx) == WEBKIT_INPUT_PURPOSE_PASSWORD)
        return;
    gchar *copy = g_strndup(text, length);
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    member_string(b, "text", copy);
    member_bool(b, "focused", TRUE);
    g_free(copy);
    member_int(b, "cursor", cursor);
    member_int(b, "selection", selection);
    json_builder_end_object(b);
    send_json('I', ((BrowserIM *)ctx)->id, b);
}
static void im_finalize(GObject *object) {
    g_free(((BrowserIM *)object)->preedit);
    G_OBJECT_CLASS(browser_im_parent_class)->finalize(object);
}
static void browser_im_class_init(BrowserIMClass *klass) {
    G_OBJECT_CLASS(klass)->finalize = im_finalize;
    WebKitInputMethodContextClass *im = WEBKIT_INPUT_METHOD_CONTEXT_CLASS(klass);
    im->get_preedit = im_preedit;
    im->filter_key_event = im_filter;
    im->notify_focus_in = im_focus_in;
    im->notify_focus_out = im_focus_out;
    im->notify_cursor_area = im_cursor;
    im->notify_surrounding = im_surrounding;
}
static void browser_im_init(BrowserIM *im) { im->preedit = g_strdup(""); }

// ---------------------------------------------------------------- clipboard
// WebKit's own copy and paste (keyboard shortcuts, menus, page script) go
// through this clipboard. Local content is forwarded to GPUI's; Zeron pushes
// the system text before any paste it forwards.
typedef struct {
    WPEClipboard parent;
    gchar *system;
} ZClipboard;
typedef struct {
    WPEClipboardClass parent_class;
} ZClipboardClass;
G_DEFINE_TYPE(ZClipboard, z_clipboard, WPE_TYPE_CLIPBOARD)
static guint clipboard_page;
static void z_clipboard_changed(WPEClipboard *clipboard, GPtrArray *formats, gboolean local,
                                WPEClipboardContent *content) {
    WPE_CLIPBOARD_CLASS(z_clipboard_parent_class)->changed(clipboard, formats, local, content);
    const char *text = local && content ? wpe_clipboard_content_get_text(content) : NULL;
    if (text)
        send_packet('C', clipboard_page, text, strlen(text));
}
static GBytes *z_clipboard_read(WPEClipboard *clipboard, const char *format) {
    ZClipboard *z = (ZClipboard *)clipboard;
    if (!z->system || !g_str_has_prefix(format, "text/plain"))
        return NULL;
    return g_bytes_new(z->system, strlen(z->system));
}
static void z_clipboard_finalize(GObject *object) {
    g_free(((ZClipboard *)object)->system);
    G_OBJECT_CLASS(z_clipboard_parent_class)->finalize(object);
}
static void z_clipboard_class_init(ZClipboardClass *klass) {
    G_OBJECT_CLASS(klass)->finalize = z_clipboard_finalize;
    WPE_CLIPBOARD_CLASS(klass)->changed = z_clipboard_changed;
    WPE_CLIPBOARD_CLASS(klass)->read = z_clipboard_read;
}
static void z_clipboard_init(ZClipboard *z) {}
// Empty text clears the copy: a paste with no text on the system clipboard
// must not insert what an earlier paste shared, possibly into another page.
static void set_system_clipboard(WPEClipboard *clipboard, const char *text) {
    ZClipboard *z = (ZClipboard *)clipboard;
    g_free(z->system);
    z->system = *text ? g_strdup(text) : NULL;
    GPtrArray *formats = g_ptr_array_new_null_terminated(2, NULL, TRUE);
    if (z->system) {
        g_ptr_array_add(formats, (gpointer)g_intern_static_string("text/plain"));
        g_ptr_array_add(formats, (gpointer)g_intern_static_string("text/plain;charset=utf-8"));
    }
    // Not local: WebKit reads it back through z_clipboard_read and it is not
    // echoed to Zeron, which would replace richer system content with text.
    WPE_CLIPBOARD_GET_CLASS(clipboard)->changed(clipboard, formats, FALSE, NULL);
    g_ptr_array_unref(formats);
}

// ---------------------------------------------------------------- vblank
// One screen per page, so each page follows the frame callbacks of the GPUI
// window showing it. WebKit expects sync callbacks on a secondary thread.
struct _ZSync {
    WPEScreenSyncObserver parent;
    guint page;
    gint active;
};
typedef struct {
    WPEScreenSyncObserverClass parent_class;
} ZSyncClass;
G_DEFINE_TYPE(ZSync, z_sync, WPE_TYPE_SCREEN_SYNC_OBSERVER)
static gboolean vsync_changed(gpointer data) {
    guint value = GPOINTER_TO_UINT(data);
    const char *json = value & 1 ? "{\"on\":true}" : "{\"on\":false}";
    send_packet('V', value >> 1, json, strlen(json));
    return G_SOURCE_REMOVE;
}
static void z_sync_set(WPEScreenSyncObserver *o, gboolean on) {
    ZSync *s = (ZSync *)o;
    g_atomic_int_set(&s->active, on);
    g_main_context_invoke(NULL, vsync_changed, GUINT_TO_POINTER(s->page << 1 | (guint)on));
}
static void z_sync_start(WPEScreenSyncObserver *o) { z_sync_set(o, TRUE); }
static void z_sync_stop(WPEScreenSyncObserver *o) { z_sync_set(o, FALSE); }
static void z_sync_class_init(ZSyncClass *klass) {
    WPE_SCREEN_SYNC_OBSERVER_CLASS(klass)->start = z_sync_start;
    WPE_SCREEN_SYNC_OBSERVER_CLASS(klass)->stop = z_sync_stop;
}
static void z_sync_init(ZSync *s) {}
static gboolean unref_idle(gpointer object) {
    g_object_unref(object);
    return G_SOURCE_REMOVE;
}
static gpointer vblank_thread(gpointer unused) {
    for (;;) {
        ZSync *sync = g_async_queue_pop(ticks);
        if ((gpointer)sync == (gpointer)&ticks)
            return NULL;
        if (g_atomic_int_get(&sync->active))
            WPE_SCREEN_SYNC_OBSERVER_GET_CLASS(sync)->sync(WPE_SCREEN_SYNC_OBSERVER(sync));
        // Finalize on the main thread, where WebKit created the observer.
        g_idle_add(unref_idle, sync);
    }
}

typedef struct {
    WPEScreen parent;
    guint page;
    ZSync *sync;
} ZScreen;
typedef struct {
    WPEScreenClass parent_class;
} ZScreenClass;
G_DEFINE_TYPE(ZScreen, z_screen, WPE_TYPE_SCREEN)
static WPEScreenSyncObserver *z_screen_get_sync_observer(WPEScreen *screen) {
    ZScreen *s = (ZScreen *)screen;
    if (!s->sync) {
        s->sync = g_object_new(z_sync_get_type(), NULL);
        s->sync->page = s->page;
    }
    // Documented transfer none, but WebKit up to 2.54 adopts the result
    // (fixed upstream by 728a05aab0e4) and would free the observer under its
    // display link when the page closes. Its display links live as long as
    // the process, so the extra reference newer versions keep costs nothing.
    return WPE_SCREEN_SYNC_OBSERVER(g_object_ref(s->sync));
}
static void z_screen_dispose(GObject *object) {
    g_clear_object(&((ZScreen *)object)->sync);
    G_OBJECT_CLASS(z_screen_parent_class)->dispose(object);
}
static void z_screen_class_init(ZScreenClass *klass) {
    G_OBJECT_CLASS(klass)->dispose = z_screen_dispose;
    WPE_SCREEN_CLASS(klass)->get_sync_observer = z_screen_get_sync_observer;
}
static void z_screen_init(ZScreen *s) {}

// ---------------------------------------------------------------- toplevel
typedef struct {
    WPEToplevel parent;
    WPEScreen *screen;
} ZToplevel;
typedef struct {
    WPEToplevelClass parent_class;
} ZToplevelClass;
G_DEFINE_TYPE(ZToplevel, z_toplevel, WPE_TYPE_TOPLEVEL)
static gboolean resize_view(WPEToplevel *t, WPEView *view, gpointer unused) {
    int w, h;
    wpe_toplevel_get_size(t, &w, &h);
    wpe_view_resized(view, w, h);
    return FALSE;
}
static gboolean z_toplevel_resize(WPEToplevel *t, int w, int h) {
    wpe_toplevel_resized(t, w, h);
    wpe_toplevel_foreach_view(t, resize_view, NULL);
    return TRUE;
}
static WPEScreen *z_toplevel_get_screen(WPEToplevel *t) { return ((ZToplevel *)t)->screen; }
static void z_toplevel_constructed(GObject *o) {
    G_OBJECT_CLASS(z_toplevel_parent_class)->constructed(o);
    if (creating)
        ((ZToplevel *)o)->screen = g_object_ref(creating->screen);
    wpe_toplevel_state_changed(WPE_TOPLEVEL(o), WPE_TOPLEVEL_STATE_ACTIVE);
}
static void z_toplevel_dispose(GObject *o) {
    g_clear_object(&((ZToplevel *)o)->screen);
    G_OBJECT_CLASS(z_toplevel_parent_class)->dispose(o);
}
static void z_toplevel_class_init(ZToplevelClass *klass) {
    G_OBJECT_CLASS(klass)->constructed = z_toplevel_constructed;
    G_OBJECT_CLASS(klass)->dispose = z_toplevel_dispose;
    WPE_TOPLEVEL_CLASS(klass)->resize = z_toplevel_resize;
    WPE_TOPLEVEL_CLASS(klass)->get_screen = z_toplevel_get_screen;
}
static void z_toplevel_init(ZToplevel *t) {}

// ---------------------------------------------------------------- frames
typedef struct {
    WPEView parent;
    Page *page;
} ZView;
typedef struct {
    WPEViewClass parent_class;
} ZViewClass;
G_DEFINE_TYPE(ZView, z_view, WPE_TYPE_VIEW)

typedef struct {
    guint id, page;
} BufferTag;
static void forget_buffer(gpointer data) {
    BufferTag *tag = data;
    if (!shutting_down) {
        gchar *json = g_strdup_printf("{\"buffer\":%u}", tag->id);
        send_packet('X', tag->page, json, strlen(json));
        g_free(json);
    }
    g_free(tag);
}
// Clamps `r` to a `width`x`height` frame; FALSE when nothing is left.
static gboolean clamp_rect(WPERectangle r, int width, int height, int *x0, int *y0, int *x1,
                           int *y1) {
    *x0 = CLAMP(r.x, 0, width);
    *y0 = CLAMP(r.y, 0, height);
    *x1 = CLAMP(r.x + r.width, 0, width);
    *y1 = CLAMP(r.y + r.height, 0, height);
    return *x1 > *x0 && *y1 > *y0;
}
static void send_frame(Page *p, guint buffer, const WPERectangle *damage, guint n, int width,
                       int height) {
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    member_int(b, "buffer", buffer);
    json_builder_set_member_name(b, "scale");
    json_builder_add_double_value(b, p->scale);
    json_builder_set_member_name(b, "damage");
    json_builder_begin_array(b);
    for (guint i = 0; i < (n ? n : 1); i++) {
        WPERectangle r = n ? damage[i] : (WPERectangle){0, 0, width, height};
        int x0, y0, x1, y1;
        if (!clamp_rect(r, width, height, &x0, &y0, &x1, &y1))
            continue;
        json_builder_begin_array(b);
        json_builder_add_int_value(b, x0);
        json_builder_add_int_value(b, y0);
        json_builder_add_int_value(b, x1 - x0);
        json_builder_add_int_value(b, y1 - y0);
        json_builder_end_array(b);
    }
    json_builder_end_array(b);
    json_builder_end_object(b);
    send_json('F', p->id, b);
}
static void close_mirror(Page *p) {
    if (p->mirror) {
        munmap(p->mirror, (size_t)p->mirror_width * p->mirror_height * 4);
        p->mirror = NULL;
    }
    if (p->mirror_fd >= 0) {
        close(p->mirror_fd);
        p->mirror_fd = -1;
        if (!shutting_down) {
            gchar *json = g_strdup_printf("{\"buffer\":%u}", p->mirror_id);
            send_packet('X', p->id, json, strlen(json));
            g_free(json);
        }
    }
}
static gboolean ensure_mirror(Page *p, int width, int height) {
    if (p->mirror && p->mirror_width == width && p->mirror_height == height)
        return TRUE;
    close_mirror(p);
    size_t size = (size_t)width * height * 4;
    int fd = memfd_create("zeron-browser-frame", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    if (fd < 0 || ftruncate(fd, size) < 0) {
        if (fd >= 0)
            close(fd);
        return FALSE;
    }
    // Zeron maps the mirror read-only and checks it is never resized.
    fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL);
    void *map = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) {
        close(fd);
        return FALSE;
    }
    p->mirror_fd = fd;
    p->mirror = map;
    p->mirror_width = width;
    p->mirror_height = height;
    p->mirror_id = next_buffer++;
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    member_int(b, "buffer", p->mirror_id);
    member_string(b, "kind", "shm");
    member_int(b, "width", width);
    member_int(b, "height", height);
    member_int(b, "stride", width * 4);
    member_int(b, "offset", 0);
    member_int(b, "fourcc", FOURCC_ARGB8888);
    json_builder_end_object(b);
    send_json_fd('B', p->id, b, fd);
    return TRUE;
}
static gboolean copy_shm(Page *p, WPEBuffer *buffer, const WPERectangle *damage, guint n) {
    WPEBufferSHM *shm = WPE_BUFFER_SHM(buffer);
    if (wpe_buffer_shm_get_format(shm) != WPE_PIXEL_FORMAT_ARGB8888)
        return FALSE;
    int width = p->mirror_width, height = p->mirror_height;
    gsize size, stride = wpe_buffer_shm_get_stride(shm);
    const guint8 *data = g_bytes_get_data(wpe_buffer_shm_get_data(shm), &size);
    if (stride < (gsize)width * 4 || size < stride * height)
        return FALSE;
    for (guint i = 0; i < n; i++) {
        int x0, y0, x1, y1;
        if (!clamp_rect(damage[i], width, height, &x0, &y0, &x1, &y1))
            continue;
        for (int y = y0; y < y1; y++)
            memcpy(p->mirror + ((size_t)y * width + x0) * 4, data + y * stride + (size_t)x0 * 4,
                   (size_t)(x1 - x0) * 4);
    }
    return TRUE;
}
static gboolean read_back(Page *p, WPEBuffer *buffer, const WPERectangle *damage, guint n);
static void state(Page *p);
// Copies a frame's damage into the mirror and sends it. The buffer itself can
// be reused as soon as this returns.
static gboolean mirror_frame(Page *p, WPEBuffer *buffer, const WPERectangle *damage, guint n) {
    int width = wpe_buffer_get_width(buffer), height = wpe_buffer_get_height(buffer);
    if (width <= 0 || height <= 0 || width > MAX_DIMENSION || height > MAX_DIMENSION)
        return FALSE;
    gboolean fresh = !p->mirror || p->mirror_width != width || p->mirror_height != height;
    if (!ensure_mirror(p, width, height))
        return FALSE;
    // A new mirror starts empty, so it needs the whole frame.
    WPERectangle all = {0, 0, width, height};
    if (fresh || !n) {
        damage = &all;
        n = 1;
    }
    gboolean copied = WPE_IS_BUFFER_SHM(buffer)       ? copy_shm(p, buffer, damage, n)
                      : WPE_IS_BUFFER_DMA_BUF(buffer) ? read_back(p, buffer, damage, n)
                                                      : FALSE;
    if (copied)
        send_frame(p, p->mirror_id, damage, n, width, height);
    return copied;
}
static gboolean contains_modifier(GArray *modifiers, guint64 modifier) {
    for (guint i = 0; modifiers && i < modifiers->len; i++)
        if (g_array_index(modifiers, guint64, i) == modifier)
            return TRUE;
    return FALSE;
}
static gboolean shares_gpu(WPEBuffer *buffer) {
    if (!WPE_IS_BUFFER_DMA_BUF(buffer))
        return FALSE;
    WPEBufferDMABuf *dmabuf = WPE_BUFFER_DMA_BUF(buffer);
    return wpe_buffer_dma_buf_get_format(dmabuf) == FOURCC_ARGB8888 &&
           wpe_buffer_dma_buf_get_n_planes(dmabuf) == 1 &&
           contains_modifier(importable, wpe_buffer_dma_buf_get_modifier(dmabuf));
}
static guint register_dmabuf(Page *p, WPEBuffer *buffer) {
    BufferTag *tag = g_object_get_data(G_OBJECT(buffer), "zeron-buffer");
    if (tag)
        return tag->id;
    tag = g_new0(BufferTag, 1);
    tag->id = next_buffer++;
    tag->page = p->id;
    g_object_set_data_full(G_OBJECT(buffer), "zeron-buffer", tag, forget_buffer);
    WPEBufferDMABuf *dmabuf = WPE_BUFFER_DMA_BUF(buffer);
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    member_int(b, "buffer", tag->id);
    member_string(b, "kind", "dmabuf");
    member_int(b, "width", wpe_buffer_get_width(buffer));
    member_int(b, "height", wpe_buffer_get_height(buffer));
    member_int(b, "stride", wpe_buffer_dma_buf_get_stride(dmabuf, 0));
    member_int(b, "offset", wpe_buffer_dma_buf_get_offset(dmabuf, 0));
    member_int(b, "fourcc", wpe_buffer_dma_buf_get_format(dmabuf));
    member_int(b, "modifier", (gint64)wpe_buffer_dma_buf_get_modifier(dmabuf));
    json_builder_end_object(b);
    send_json_fd('B', p->id, b, wpe_buffer_dma_buf_get_fd(dmabuf, 0));
    return tag->id;
}
static void present(Page *p) {
    WPEBuffer *buffer = g_steal_pointer(&p->rendered);
    if (buffer) {
        wpe_view_buffer_rendered(p->view, buffer);
        g_object_unref(buffer);
    }
}
static void release_held(Page *p, guint id) {
    WPEBuffer *buffer = g_hash_table_lookup(p->held, GUINT_TO_POINTER(id));
    if (buffer) {
        wpe_view_buffer_released(p->view, buffer);
        g_hash_table_remove(p->held, GUINT_TO_POINTER(id));
    }
}
static gboolean z_view_render_buffer(WPEView *view, WPEBuffer *buffer, const WPERectangle *damage,
                                     guint n, GError **error) {
    Page *p = ((ZView *)view)->page;
    if (!p) {
        g_set_error_literal(error, WPE_VIEW_ERROR, WPE_VIEW_ERROR_RENDER_FAILED, "page closed");
        return FALSE;
    }
    // WebKit renders again only after buffer_rendered; never strand a frame.
    present(p);
    if (!p->readback && shares_gpu(buffer)) {
        guint id = register_dmabuf(p, buffer);
        g_hash_table_insert(p->held, GUINT_TO_POINTER(id), g_object_ref(buffer));
        send_frame(p, id, damage, n, wpe_buffer_get_width(buffer), wpe_buffer_get_height(buffer));
    } else {
        if (!mirror_frame(p, buffer, damage, n)) {
            // WebKit renders again but would never reuse this buffer.
            wpe_view_buffer_released(view, buffer);
            // No frame reaches Zeron: say why once, rather than show nothing.
            if (!p->error) {
                p->error = g_strdup("This page's frames could not be shown.");
                state(p);
            }
            g_set_error_literal(error, WPE_VIEW_ERROR, WPE_VIEW_ERROR_RENDER_FAILED,
                                "could not deliver the frame");
            return FALSE;
        }
        wpe_view_buffer_released(view, buffer);
    }
    p->rendered = g_object_ref(buffer);
    return TRUE;
}
static void z_view_set_cursor(WPEView *view, const char *name) {
    Page *p = ((ZView *)view)->page;
    if (p && name)
        send_packet('K', p->id, name, strlen(name));
}
static void z_view_constructed(GObject *o) {
    G_OBJECT_CLASS(z_view_parent_class)->constructed(o);
    ((ZView *)o)->page = creating;
}
static void z_view_class_init(ZViewClass *klass) {
    G_OBJECT_CLASS(klass)->constructed = z_view_constructed;
    WPE_VIEW_CLASS(klass)->render_buffer = z_view_render_buffer;
    WPE_VIEW_CLASS(klass)->set_cursor_from_name = z_view_set_cursor;
}
static void z_view_init(ZView *v) {}

// ---------------------------------------------------------------- display
typedef struct {
    WPEDisplay parent;
    GPtrArray *screens;
    WPEDRMDevice *drm;
    struct gbm_device *gbm;
    int gbm_fd;
    gboolean probed;
    EGLDisplay egl;
    GArray *modifiers; // guint64 AR24 layouts WebKit renders into
    gboolean shared;   // GPUI imports all of them
    EGLContext gl;     // reads back frames GPUI cannot import
    WPEKeymap *keymap;
    WPEClipboard *clipboard;
} ZDisplay;
typedef struct {
    WPEDisplayClass parent_class;
} ZDisplayClass;
G_DEFINE_TYPE(ZDisplay, z_display, WPE_TYPE_DISPLAY)
// The primary node that shares a render node's device, as WPE requires both.
static gchar *primary_node(const char *render) {
    struct stat st;
    if (stat(render, &st) < 0 || !S_ISCHR(st.st_mode))
        return NULL;
    gchar *sys = g_strdup_printf("/sys/dev/char/%u:%u/device/drm", major(st.st_rdev),
                                 minor(st.st_rdev));
    GDir *dir = g_dir_open(sys, 0, NULL);
    g_free(sys);
    gchar *primary = NULL;
    for (const char *name; dir && !primary && (name = g_dir_read_name(dir));)
        if (g_str_has_prefix(name, "card"))
            primary = g_build_filename("/dev/dri", name, NULL);
    if (dir)
        g_dir_close(dir);
    return primary;
}
static gboolean has_extension(EGLDisplay egl, const char *name) {
    const char *list = eglQueryString(egl, EGL_EXTENSIONS);
    size_t length = strlen(name);
    for (const char *at = list; at && (at = strstr(at, name)); at += length)
        if ((at == list || at[-1] == ' ') && (at[length] == ' ' || !at[length]))
            return TRUE;
    return FALSE;
}
// The AR24 layouts WebKit can render into on this GPU: EGL renders to them and
// GBM allocates them in one plane, as Zeron takes one fd per buffer. GPUs
// differ: NVIDIA renders only to its tiled layouts.
static GArray *renderable_modifiers(ZDisplay *d) {
    GArray *modifiers = g_array_new(FALSE, FALSE, sizeof(guint64));
    if (!has_extension(d->egl, "EGL_EXT_image_dma_buf_import_modifiers"))
        return modifiers;
    PFNEGLQUERYDMABUFMODIFIERSEXTPROC query =
        (PFNEGLQUERYDMABUFMODIFIERSEXTPROC)eglGetProcAddress("eglQueryDmaBufModifiersEXT");
    EGLint n = 0;
    if (!query || !query(d->egl, FOURCC_ARGB8888, 0, NULL, NULL, &n) || n <= 0)
        return modifiers;
    EGLuint64KHR *all = g_new(EGLuint64KHR, n);
    EGLBoolean *external = g_new(EGLBoolean, n);
    if (query(d->egl, FOURCC_ARGB8888, n, all, external, &n))
        for (EGLint i = 0; i < n; i++) {
            guint64 modifier = all[i];
            struct gbm_bo *bo = external[i] ? NULL
                                            : gbm_bo_create_with_modifiers2(
                                                  d->gbm, 64, 64, FOURCC_ARGB8888, &modifier, 1,
                                                  GBM_BO_USE_RENDERING);
            if (bo && gbm_bo_get_plane_count(bo) == 1)
                g_array_append_val(modifiers, modifier);
            if (bo)
                gbm_bo_destroy(bo);
        }
    g_free(all);
    g_free(external);
    return modifiers;
}
static void close_gbm(ZDisplay *d) {
    g_clear_pointer(&d->gbm, gbm_device_destroy);
    if (d->gbm_fd >= 0) {
        close(d->gbm_fd);
        d->gbm_fd = -1;
    }
}
// WebKit aborts its web process when GBM EGL cannot initialize, so only
// advertise the GPU after initializing EGL on it here.
static void probe_gpu(ZDisplay *d) {
    if (d->probed)
        return;
    d->probed = TRUE;
    if (!render_node)
        return;
    gchar *primary = primary_node(render_node);
    d->gbm_fd = open(render_node, O_RDWR | O_CLOEXEC);
    d->gbm = d->gbm_fd >= 0 ? gbm_create_device(d->gbm_fd) : NULL;
    EGLDisplay egl =
        d->gbm ? eglGetPlatformDisplay(EGL_PLATFORM_GBM_KHR, d->gbm, NULL) : EGL_NO_DISPLAY;
    EGLint major = 0, minor = 0;
    if (!primary || egl == EGL_NO_DISPLAY || !eglInitialize(egl, &major, &minor)) {
        g_printerr("zeron-wpe: no usable EGL on %s (0x%x), using shared memory\n", render_node,
                   eglGetError());
        if (egl != EGL_NO_DISPLAY)
            eglTerminate(egl);
        close_gbm(d);
        g_free(primary);
        return;
    }
    d->egl = egl;
    GArray *renderable = renderable_modifiers(d);
    if (!renderable->len) {
        g_printerr("zeron-wpe: %s renders no shareable AR24 layout, using shared memory\n",
                   render_node);
        g_array_unref(renderable);
        eglTerminate(egl);
        d->egl = EGL_NO_DISPLAY;
        close_gbm(d);
        g_free(primary);
        return;
    }
    // Zero copy needs a layout GPUI imports; GL reads the others back.
    d->modifiers = g_array_new(FALSE, FALSE, sizeof(guint64));
    for (guint i = 0; i < renderable->len; i++) {
        guint64 modifier = g_array_index(renderable, guint64, i);
        if (contains_modifier(importable, modifier))
            g_array_append_val(d->modifiers, modifier);
    }
    d->shared = d->modifiers->len > 0;
    if (!d->shared) {
        g_printerr("zeron-wpe: Zeron imports none of the layouts %s renders, reading frames back\n",
                   render_node);
        g_array_unref(d->modifiers);
        d->modifiers = g_array_ref(renderable);
    }
    g_array_unref(renderable);
    d->drm = wpe_drm_device_new(primary, render_node);
    g_free(primary);
}
static gboolean z_display_connect(WPEDisplay *display, GError **error) { return TRUE; }
static WPEView *z_display_create_view(WPEDisplay *display) {
    return g_object_new(z_view_get_type(), "display", display, NULL);
}
static WPEToplevel *z_display_create_toplevel(WPEDisplay *display, guint max_views) {
    return g_object_new(z_toplevel_get_type(), "display", display, "max-views", max_views, NULL);
}
static gpointer z_display_get_egl_display(WPEDisplay *display, GError **error) {
    ZDisplay *d = (ZDisplay *)display;
    probe_gpu(d);
    if (!d->egl)
        g_set_error_literal(error, WPE_EGL_ERROR, WPE_EGL_ERROR_NOT_AVAILABLE,
                            "shared memory only");
    return d->egl;
}
static WPEDRMDevice *z_display_get_drm_device(WPEDisplay *display) {
    ZDisplay *d = (ZDisplay *)display;
    probe_gpu(d);
    return d->drm;
}
// GBM picks the best of these layouts for each buffer WebKit allocates.
static WPEBufferFormats *z_display_get_preferred_buffer_formats(WPEDisplay *display) {
    ZDisplay *d = (ZDisplay *)display;
    probe_gpu(d);
    if (!d->drm)
        return NULL;
    WPEBufferFormatsBuilder *builder = wpe_buffer_formats_builder_new(d->drm);
    wpe_buffer_formats_builder_append_group(builder, d->drm, WPE_BUFFER_FORMAT_USAGE_RENDERING);
    for (guint i = 0; i < d->modifiers->len; i++)
        wpe_buffer_formats_builder_append_format(builder, FOURCC_ARGB8888,
                                                 g_array_index(d->modifiers, guint64, i));
    return wpe_buffer_formats_builder_end(builder);
}
// GBM cannot map every layout (none of NVIDIA's tiled ones), but GL reads back
// whatever EGL rendered. GL starts from the first row in memory, as the mirror.
static gboolean read_back(Page *p, WPEBuffer *buffer, const WPERectangle *damage, guint n) {
    ZDisplay *d = (ZDisplay *)display;
    static PFNGLEGLIMAGETARGETRENDERBUFFERSTORAGEOESPROC storage;
    if (!d->egl)
        return FALSE;
    if (!d->gl) {
        static const EGLint attributes[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
        eglBindAPI(EGL_OPENGL_ES_API);
        d->gl = eglCreateContext(d->egl, EGL_NO_CONFIG_KHR, EGL_NO_CONTEXT, attributes);
        storage = (PFNGLEGLIMAGETARGETRENDERBUFFERSTORAGEOESPROC)eglGetProcAddress(
            "glEGLImageTargetRenderbufferStorageOES");
    }
    if (!d->gl || !storage || !eglMakeCurrent(d->egl, EGL_NO_SURFACE, EGL_NO_SURFACE, d->gl)) {
        g_printerr("zeron-wpe: no GL context to read frames back (0x%x)\n", eglGetError());
        return FALSE;
    }
    GError *error = NULL;
    EGLImage image = wpe_buffer_import_to_egl_image(buffer, &error);
    if (!image) {
        g_printerr("zeron-wpe: frame readback failed: %s\n", error ? error->message : "?");
        g_clear_error(&error);
        eglMakeCurrent(d->egl, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        return FALSE;
    }
    GLuint renderbuffer, framebuffer;
    glGenRenderbuffers(1, &renderbuffer);
    glBindRenderbuffer(GL_RENDERBUFFER, renderbuffer);
    storage(GL_RENDERBUFFER, image);
    glGenFramebuffers(1, &framebuffer);
    glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, renderbuffer);
    gboolean read = glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE;
    int width = p->mirror_width, height = p->mirror_height;
    for (guint i = 0; read && i < n; i++) {
        int x0, y0, x1, y1;
        if (!clamp_rect(damage[i], width, height, &x0, &y0, &x1, &y1))
            continue;
        int w = x1 - x0;
        guint8 *rgba = g_malloc((size_t)w * (y1 - y0) * 4);
        glReadPixels(x0, y0, w, y1 - y0, GL_RGBA, GL_UNSIGNED_BYTE, rgba);
        for (int y = y0; y < y1; y++) {
            const guint8 *from = rgba + (size_t)(y - y0) * w * 4;
            guint8 *to = p->mirror + ((size_t)y * width + x0) * 4;
            for (int x = 0; x < w; x++, from += 4, to += 4) {
                to[0] = from[2];
                to[1] = from[1];
                to[2] = from[0];
                to[3] = from[3];
            }
        }
        g_free(rgba);
    }
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    glDeleteFramebuffers(1, &framebuffer);
    glDeleteRenderbuffers(1, &renderbuffer);
    read = read && glGetError() == GL_NO_ERROR;
    eglMakeCurrent(d->egl, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    return read;
}
static guint z_display_get_n_screens(WPEDisplay *display) {
    return ((ZDisplay *)display)->screens->len;
}
static WPEScreen *z_display_get_screen(WPEDisplay *display, guint i) {
    GPtrArray *screens = ((ZDisplay *)display)->screens;
    return i < screens->len ? g_ptr_array_index(screens, i) : NULL;
}
static WPEKeymap *z_display_get_keymap(WPEDisplay *display) {
    ZDisplay *d = (ZDisplay *)display;
    if (!d->keymap)
        d->keymap = wpe_keymap_xkb_new();
    return d->keymap;
}
static WPEClipboard *z_display_get_clipboard(WPEDisplay *display) {
    ZDisplay *d = (ZDisplay *)display;
    if (!d->clipboard)
        d->clipboard = g_object_new(z_clipboard_get_type(), "display", display, NULL);
    return d->clipboard;
}
// No use_explicit_sync: without it WebKit waits for each frame's rendering
// fence before rendering a buffer here, and Zeron copies frames without one.
static void z_display_class_init(ZDisplayClass *klass) {
    WPEDisplayClass *c = WPE_DISPLAY_CLASS(klass);
    c->connect = z_display_connect;
    c->create_view = z_display_create_view;
    c->create_toplevel = z_display_create_toplevel;
    c->get_egl_display = z_display_get_egl_display;
    c->get_drm_device = z_display_get_drm_device;
    c->get_preferred_buffer_formats = z_display_get_preferred_buffer_formats;
    c->get_n_screens = z_display_get_n_screens;
    c->get_screen = z_display_get_screen;
    c->get_keymap = z_display_get_keymap;
    c->get_clipboard = z_display_get_clipboard;
}
static void z_display_init(ZDisplay *d) {
    d->gbm_fd = -1;
    d->screens = g_ptr_array_new_with_free_func(g_object_unref);
}

// ---------------------------------------------------------------- pages
static void state(Page *p) {
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    member_string(b, "url", webkit_web_view_get_uri(p->web));
    member_string(b, "title", webkit_web_view_get_title(p->web) ?: "");
    member_string(b, "error", p->error);
    member_bool(b, "loading", webkit_web_view_is_loading(p->web));
    member_bool(b, "can_back", webkit_web_view_can_go_back(p->web));
    member_bool(b, "can_forward", webkit_web_view_can_go_forward(p->web));
    json_builder_end_object(b);
    send_json('S', p->id, b);
}
static void changed(GObject *web, GParamSpec *spec, Page *p) { state(p); }
static void loaded(WebKitWebView *web, WebKitLoadEvent event, Page *p) {
    if (event == WEBKIT_LOAD_STARTED)
        g_clear_pointer(&p->error, g_free);
    state(p);
}
static gboolean failed(WebKitWebView *web, WebKitLoadEvent event, const char *uri, GError *error,
                       Page *p) {
    if (g_error_matches(error, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED))
        return TRUE;
    g_free(p->error);
    p->error = g_strdup(error->message);
    state(p);
    return TRUE;
}
static void terminated(WebKitWebView *web, WebKitWebProcessTerminationReason reason, Page *p) {
    g_free(p->error);
    p->error = g_strdup("The browser process stopped. Reload to try again.");
    state(p);
}
static gboolean allowed(const char *uri) {
    GUri *u = uri ? g_uri_parse(uri, G_URI_FLAGS_NONE, NULL) : NULL;
    if (!u)
        return FALSE;
    const char *scheme = g_uri_get_scheme(u);
    gboolean ok = scheme && (!strcmp(scheme, "http") || !strcmp(scheme, "https")) &&
                  g_uri_get_host(u) && !g_uri_get_userinfo(u);
    g_uri_unref(u);
    return ok;
}
static gboolean policy(WebKitWebView *web, WebKitPolicyDecision *decision,
                       WebKitPolicyDecisionType type, Page *p) {
    if (type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION ||
        type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
        WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(
            WEBKIT_NAVIGATION_POLICY_DECISION(decision));
        const char *uri = webkit_uri_request_get_uri(webkit_navigation_action_get_request(action));
        if (!allowed(uri)) {
            webkit_policy_decision_ignore(decision);
            return TRUE;
        }
        if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
            if (webkit_navigation_action_is_user_gesture(action))
                send_packet('N', p->id, uri, strlen(uri));
            webkit_policy_decision_ignore(decision);
            return TRUE;
        }
    }
    return FALSE;
}
static gboolean permission(WebKitWebView *web, WebKitPermissionRequest *request, Page *p) {
    webkit_permission_request_deny(request);
    return TRUE;
}
static JsonBuilder *menu_builder(double x, double y) {
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    json_builder_set_member_name(b, "x");
    json_builder_add_double_value(b, x);
    json_builder_set_member_name(b, "y");
    json_builder_add_double_value(b, y);
    json_builder_set_member_name(b, "items");
    json_builder_begin_array(b);
    return b;
}
static void menu_item(JsonBuilder *b, const char *label, const char *action, gboolean enabled,
                      gboolean selected) {
    json_builder_begin_object(b);
    member_string(b, "label", label);
    member_string(b, "action", action);
    member_bool(b, "enabled", enabled);
    member_bool(b, "selected", selected);
    json_builder_end_object(b);
}
static void send_menu(Page *p, JsonBuilder *b) {
    json_builder_end_array(b);
    json_builder_end_object(b);
    send_json('M', p->id, b);
}
static gboolean context_menu(WebKitWebView *web, WebKitContextMenu *menu, WebKitHitTestResult *hit,
                             Page *p) {
    g_free(p->context_link);
    p->context_link = NULL;
    JsonBuilder *b = menu_builder(p->pointer_x, p->pointer_y);
    if (webkit_hit_test_result_context_is_link(hit)) {
        p->context_link = g_strdup(webkit_hit_test_result_get_link_uri(hit));
        menu_item(b, "Open link in new tab", "open-link", allowed(p->context_link), FALSE);
        menu_item(b, "Copy link address", "copy-link", TRUE, FALSE);
    }
    menu_item(b, "Copy", "copy",
              webkit_hit_test_result_context_is_selection(hit) ||
                  webkit_hit_test_result_context_is_editable(hit),
              FALSE);
    if (webkit_hit_test_result_context_is_editable(hit))
        menu_item(b, "Paste", "paste", TRUE, FALSE);
    menu_item(b, "Select all", "select-all", TRUE, FALSE);
    menu_item(b, "Back", "back", webkit_web_view_can_go_back(web), FALSE);
    menu_item(b, "Forward", "forward", webkit_web_view_can_go_forward(web), FALSE);
    menu_item(b, "Reload", "reload", TRUE, FALSE);
    send_menu(p, b);
    return TRUE;
}
static gboolean option_menu(WebKitWebView *web, WebKitOptionMenu *menu, WebKitRectangle *rect,
                            Page *p) {
    g_set_object(&p->options, menu);
    JsonBuilder *b = menu_builder(rect->x, rect->y + rect->height);
    for (guint i = 0; i < webkit_option_menu_get_n_items(menu); i++) {
        WebKitOptionMenuItem *item = webkit_option_menu_get_item(menu, i);
        gchar *action = g_strdup_printf("option:%u", i);
        menu_item(b, webkit_option_menu_item_get_label(item), action,
                  webkit_option_menu_item_is_enabled(item) &&
                      !webkit_option_menu_item_is_group_label(item),
                  webkit_option_menu_item_is_selected(item));
        g_free(action);
    }
    send_menu(p, b);
    return TRUE;
}
static void download(WebKitNetworkSession *session, WebKitDownload *item, gpointer data) {
    webkit_download_cancel(item);
}
static void toplevel_changed(WPEView *view, GParamSpec *spec, Page *p) {
    WPEToplevel *toplevel = wpe_view_get_toplevel(view);
    if (!toplevel)
        return;
    int w, h;
    wpe_toplevel_get_size(toplevel, &w, &h);
    if (w && h)
        wpe_view_resized(view, w, h);
    if (p->visible)
        wpe_view_map(view);
}
static void free_page(gpointer data) {
    Page *p = data;
    if (p->options) {
        webkit_option_menu_close(p->options);
        g_clear_object(&p->options);
    }
    // WebKit may outlive this page briefly; nothing may call back into it.
    ((ZView *)p->view)->page = NULL;
    g_signal_handlers_disconnect_by_data(p->web, p);
    g_signal_handlers_disconnect_by_data(p->view, p);
    g_clear_object(&p->rendered);
    g_hash_table_destroy(p->held);
    close_mirror(p);
    g_object_unref(p->web);
    wpe_display_screen_removed(display, p->screen);
    g_ptr_array_remove(((ZDisplay *)display)->screens, p->screen);
    g_free(p->error);
    g_free(p->context_link);
    g_free(p);
}
static Page *new_page(guint id) {
    Page *p = g_new0(Page, 1);
    p->id = id;
    p->width = 800;
    p->height = 600;
    p->scale = 1;
    p->visible = TRUE;
    p->mirror_fd = -1;
    p->held = g_hash_table_new_full(g_direct_hash, g_direct_equal, NULL, g_object_unref);
    p->screen = g_object_new(z_screen_get_type(), "id", id, NULL);
    ((ZScreen *)p->screen)->page = id;
    wpe_screen_set_scale(p->screen, 1);
    wpe_screen_set_refresh_rate(p->screen, 60000);
    wpe_screen_set_size(p->screen, 1920, 1080);
    wpe_screen_set_physical_size(p->screen, 527, 296);
    g_ptr_array_add(((ZDisplay *)display)->screens, p->screen);
    wpe_display_screen_added(display, p->screen);
    creating = p;
    p->web = g_object_new(WEBKIT_TYPE_WEB_VIEW, "display", display, "network-session", session,
                          "settings", settings, NULL);
    creating = NULL;
    p->view = webkit_web_view_get_wpe_view(p->web);
    BrowserIM *im = g_object_new(browser_im_get_type(), NULL);
    im->id = id;
    webkit_web_view_set_input_method_context(p->web, WEBKIT_INPUT_METHOD_CONTEXT(im));
    g_object_unref(im);
    g_signal_connect(p->web, "notify::uri", G_CALLBACK(changed), p);
    g_signal_connect(p->web, "notify::title", G_CALLBACK(changed), p);
    g_signal_connect(p->web, "notify::is-loading", G_CALLBACK(changed), p);
    g_signal_connect(p->web, "load-changed", G_CALLBACK(loaded), p);
    g_signal_connect(p->web, "load-failed", G_CALLBACK(failed), p);
    g_signal_connect(p->web, "web-process-terminated", G_CALLBACK(terminated), p);
    g_signal_connect(p->web, "decide-policy", G_CALLBACK(policy), p);
    g_signal_connect(p->web, "permission-request", G_CALLBACK(permission), p);
    g_signal_connect(p->web, "context-menu", G_CALLBACK(context_menu), p);
    g_signal_connect(p->web, "show-option-menu", G_CALLBACK(option_menu), p);
    g_signal_connect(p->view, "notify::toplevel", G_CALLBACK(toplevel_changed), p);
    WPEToplevel *toplevel = wpe_view_get_toplevel(p->view);
    if (toplevel && !((ZToplevel *)toplevel)->screen) {
        ((ZToplevel *)toplevel)->screen = g_object_ref(p->screen);
        wpe_toplevel_screen_changed(toplevel);
    }
    if (toplevel)
        wpe_toplevel_resize(toplevel, p->width, p->height);
    toplevel_changed(p->view, NULL, p);
    g_hash_table_insert(pages, GUINT_TO_POINTER(id), p);
    return p;
}

// ---------------------------------------------------------------- commands
static double number(JsonObject *o, const char *key) {
    JsonNode *node = json_object_get_member(o, key);
    return node && JSON_NODE_HOLDS_VALUE(node) && json_node_get_value_type(node) != G_TYPE_STRING
               ? json_node_get_double(node)
               : 0;
}
static const char *string(JsonObject *o, const char *key) {
    JsonNode *node = json_object_get_member(o, key);
    return node && JSON_NODE_HOLDS_VALUE(node) && json_node_get_value_type(node) == G_TYPE_STRING
               ? json_node_get_string(node)
               : "";
}
static guint32 event_time(void) { return (guint32)(g_get_monotonic_time() / 1000); }
// Zeron sends GDK modifier masks, shared with the WebKitGTK helper.
static WPEModifiers modifiers(JsonObject *o) {
    guint mods = number(o, "mods"), wpe = 0;
    if (mods & 1)
        wpe |= WPE_MODIFIER_KEYBOARD_SHIFT;
    if (mods & 4)
        wpe |= WPE_MODIFIER_KEYBOARD_CONTROL;
    if (mods & 8)
        wpe |= WPE_MODIFIER_KEYBOARD_ALT;
    if (mods & (1u << 26))
        wpe |= WPE_MODIFIER_KEYBOARD_META;
    return wpe;
}
static guint button_modifier(guint button) {
    return button >= 1 && button <= 5 ? WPE_MODIFIER_POINTER_BUTTON1 << (button - 1) : 0;
}
static void dispatch(Page *p, WPEEvent *event) {
    wpe_view_event(p->view, event);
    wpe_event_unref(event);
}
static void pointer_event(Page *p, JsonObject *o, const char *cmd) {
    double x = number(o, "x"), y = number(o, "y");
    p->pointer_x = x;
    p->pointer_y = y;
    WPEModifiers mods = modifiers(o);
    if (!strcmp(cmd, "move") || !strcmp(cmd, "enter")) {
        WPEEventType type = !strcmp(cmd, "move") ? WPE_EVENT_POINTER_MOVE : WPE_EVENT_POINTER_ENTER;
        dispatch(p, wpe_event_pointer_move_new(type, p->view, WPE_INPUT_SOURCE_MOUSE, event_time(),
                                               mods | p->buttons, x, y, 0, 0));
    } else if (!strcmp(cmd, "leave")) {
        dispatch(p, wpe_event_pointer_move_new(WPE_EVENT_POINTER_LEAVE, p->view,
                                               WPE_INPUT_SOURCE_MOUSE, event_time(),
                                               mods | p->buttons, -1, -1, 0, 0));
    } else {
        guint button = number(o, "button");
        gboolean down = !strcmp(cmd, "down");
        if (down) {
            p->buttons |= button_modifier(button);
            wpe_view_focus_in(p->view);
        } else
            p->buttons &= ~button_modifier(button);
        guint count = MAX(1, (guint)number(o, "count"));
        dispatch(p, wpe_event_pointer_button_new(down ? WPE_EVENT_POINTER_DOWN : WPE_EVENT_POINTER_UP,
                                                 p->view, WPE_INPUT_SOURCE_MOUSE, event_time(),
                                                 mods | p->buttons, button, x, y,
                                                 down ? count : 0));
    }
}
static void scroll_event(Page *p, JsonObject *o) {
    gboolean precise = json_object_get_boolean_member_with_default(o, "precise", TRUE);
    gboolean stop = json_object_get_boolean_member_with_default(o, "stop", FALSE);
    dispatch(p, wpe_event_scroll_new(p->view,
                                     precise ? WPE_INPUT_SOURCE_TOUCHPAD : WPE_INPUT_SOURCE_MOUSE,
                                     event_time(), modifiers(o) | p->buttons, number(o, "dx"),
                                     number(o, "dy"), precise, stop, number(o, "x"),
                                     number(o, "y")));
}
static guint keyval_for(const char *key) {
    xkb_keysym_t sym = xkb_keysym_from_name(key, XKB_KEYSYM_NO_FLAGS);
    if (sym == XKB_KEY_NoSymbol) {
        gunichar u = g_utf8_get_char_validated(key, -1);
        if (u != (gunichar)-1 && u != (gunichar)-2 && u)
            sym = wpe_unicode_to_keyval(u);
    }
    return sym;
}
static void key_event(Page *p, JsonObject *o, gboolean down) {
    guint keyval = keyval_for(string(o, "key"));
    if (keyval == XKB_KEY_NoSymbol)
        return;
    guint keycode = 0, n = 0;
    WPEKeymapEntry *entries = NULL;
    if (wpe_keymap_get_entries_for_keyval(wpe_display_get_keymap(display), keyval, &entries, &n) &&
        n)
        keycode = entries[0].keycode;
    g_free(entries);
    dispatch(p, wpe_event_keyboard_new(down ? WPE_EVENT_KEYBOARD_KEY_DOWN : WPE_EVENT_KEYBOARD_KEY_UP,
                                       p->view, WPE_INPUT_SOURCE_KEYBOARD, event_time(),
                                       modifiers(o) | p->buttons, keycode, keyval));
}
static void evaluated(GObject *web, GAsyncResult *result, gpointer data) {
    guint id = GPOINTER_TO_UINT(data);
    GError *error = NULL;
    JSCValue *v = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(web), result, &error);
    gchar *s = v ? jsc_value_to_json(v, 0) : NULL;
    send_packet('J', id, s ?: "null", strlen(s ?: "null"));
    g_free(s);
    g_clear_object(&v);
    g_clear_error(&error);
}
static void shutdown_ready(GObject *manager, GAsyncResult *result, gpointer unused) {
    GError *error = NULL;
    GList *cookies =
        webkit_cookie_manager_get_cookies_finish(WEBKIT_COOKIE_MANAGER(manager), result, &error);
    g_list_free_full(cookies, (GDestroyNotify)soup_cookie_free);
    g_clear_error(&error);
    g_main_loop_quit(loop);
}
static void begin_shutdown(void) {
    if (shutting_down)
        return;
    shutting_down = TRUE;
    g_hash_table_remove_all(pages);
    // Let WebKit process the pending cookie operations before leaving its loop.
    webkit_cookie_manager_get_cookies(webkit_network_session_get_cookie_manager(session),
                                      "http://localhost/", NULL, shutdown_ready, NULL);
}
static void resize(Page *p, JsonObject *o) {
    int w = CLAMP(number(o, "width"), 1, MAX_DIMENSION),
        h = CLAMP(number(o, "height"), 1, MAX_DIMENSION);
    double scale = CLAMP(number(o, "scale"), 0.5, 4);
    if (w == p->width && h == p->height && scale == p->scale)
        return;
    WPEToplevel *toplevel = wpe_view_get_toplevel(p->view);
    if (!toplevel)
        return;
    if (scale != p->scale) {
        wpe_screen_set_scale(p->screen, scale);
        wpe_toplevel_scale_changed(toplevel, scale);
    }
    p->width = w;
    p->height = h;
    p->scale = scale;
    // WebKit lays out in logical pixels and renders them at the device scale.
    // Round up: GPUI clips a page that overflows by a fraction of a pixel,
    // where rounding down would leave a seam along its right and bottom edges.
    // Never past the largest frame Zeron takes, though.
    int most = (int)(MAX_DIMENSION / scale);
    wpe_toplevel_resize(toplevel, CLAMP((int)ceil(w / scale - 0.01), 1, most),
                        CLAMP((int)ceil(h / scale - 0.01), 1, most));
}
static void command(JsonObject *o) {
    guint id = number(o, "id");
    const char *cmd = string(o, "cmd");
    if (!strcmp(cmd, "shutdown")) {
        begin_shutdown();
        return;
    }
    if (shutting_down)
        return;
    if (!strcmp(cmd, "clipboard")) {
        set_system_clipboard(wpe_display_get_clipboard(display), string(o, "text"));
        return;
    }
    Page *p = g_hash_table_lookup(pages, GUINT_TO_POINTER(id));
    if (!strcmp(cmd, "create")) {
        if (!p)
            new_page(id);
        return;
    }
    if (!p)
        return;
    // WebKit's copies go to the page the user last pressed a key or button in.
    if (!strcmp(cmd, "key_down") || !strcmp(cmd, "down"))
        clipboard_page = id;
    if (!strcmp(cmd, "close")) {
        g_hash_table_remove(pages, GUINT_TO_POINTER(id));
    } else if (!strcmp(cmd, "vblank")) {
        ZSync *sync = ((ZScreen *)p->screen)->sync;
        if (sync && g_atomic_int_get(&sync->active))
            g_async_queue_push(ticks, g_object_ref(sync));
    } else if (!strcmp(cmd, "presented")) {
        present(p);
    } else if (!strcmp(cmd, "released")) {
        release_held(p, number(o, "buffer"));
    } else if (!strcmp(cmd, "readback")) {
        // Zeron could not import this frame: resend it, and every later one,
        // through shared memory.
        guint buffer = number(o, "buffer");
        WPEBuffer *held = g_hash_table_lookup(p->held, GUINT_TO_POINTER(buffer));
        p->readback = TRUE;
        if (!held || !mirror_frame(p, held, NULL, 0)) {
            if (held)
                g_printerr("zeron-wpe: page %u has no usable frame path\n", p->id);
            // No frame comes to acknowledge: let WebKit render the next.
            present(p);
        }
        release_held(p, buffer);
    } else if (!strcmp(cmd, "refresh")) {
        wpe_screen_set_refresh_rate(p->screen, CLAMP(number(o, "mhz"), 1000, 1000000));
    } else if (!strcmp(cmd, "load")) {
        const char *url = string(o, "url");
        if (allowed(url))
            webkit_web_view_load_uri(p->web, url);
    } else if (!strcmp(cmd, "dismiss-menu")) {
        if (p->options) {
            webkit_option_menu_close(p->options);
            g_clear_object(&p->options);
        }
    } else if (g_str_has_prefix(cmd, "option:")) {
        if (p->options) {
            guint index = g_ascii_strtoull(cmd + 7, NULL, 10);
            if (index < webkit_option_menu_get_n_items(p->options))
                webkit_option_menu_activate_item(p->options, index);
            webkit_option_menu_close(p->options);
            g_clear_object(&p->options);
        }
    } else if (!strcmp(cmd, "open-link")) {
        if (p->context_link && allowed(p->context_link))
            send_packet('N', p->id, p->context_link, strlen(p->context_link));
    } else if (!strcmp(cmd, "copy-link")) {
        if (p->context_link)
            send_packet('C', p->id, p->context_link, strlen(p->context_link));
    } else if (!strcmp(cmd, "copy"))
        webkit_web_view_execute_editing_command(p->web, WEBKIT_EDITING_COMMAND_COPY);
    else if (!strcmp(cmd, "cut"))
        webkit_web_view_execute_editing_command(p->web, WEBKIT_EDITING_COMMAND_CUT);
    else if (!strcmp(cmd, "paste"))
        webkit_web_view_execute_editing_command(p->web, WEBKIT_EDITING_COMMAND_PASTE);
    else if (!strcmp(cmd, "select-all"))
        webkit_web_view_execute_editing_command(p->web, WEBKIT_EDITING_COMMAND_SELECT_ALL);
    else if (!strcmp(cmd, "reload"))
        webkit_web_view_reload(p->web);
    else if (!strcmp(cmd, "back"))
        webkit_web_view_go_back(p->web);
    else if (!strcmp(cmd, "forward"))
        webkit_web_view_go_forward(p->web);
    else if (!strcmp(cmd, "commit") || !strcmp(cmd, "preedit") || !strcmp(cmd, "unmark")) {
        BrowserIM *im = (BrowserIM *)webkit_web_view_get_input_method_context(p->web);
        if (!strcmp(cmd, "preedit")) {
            if (!*im->preedit)
                g_signal_emit_by_name(im, "preedit-started");
            g_free(im->preedit);
            im->preedit = g_strdup(string(o, "text"));
            im->cursor = g_utf8_strlen(im->preedit, -1);
            g_signal_emit_by_name(im, "preedit-changed");
        } else {
            if (!strcmp(cmd, "commit"))
                g_signal_emit_by_name(im, "committed", string(o, "text"));
            g_free(im->preedit);
            im->preedit = g_strdup("");
            im->cursor = 0;
            g_signal_emit_by_name(im, "preedit-changed");
            g_signal_emit_by_name(im, "preedit-finished");
        }
    } else if (!strcmp(cmd, "text"))
        webkit_web_view_execute_editing_command_with_argument(p->web, "InsertText",
                                                              string(o, "text"));
    else if (!strcmp(cmd, "resize"))
        resize(p, o);
    else if (!strcmp(cmd, "visible")) {
        p->visible = number(o, "value") != 0;
        if (p->visible)
            wpe_view_map(p->view);
        else
            wpe_view_unmap(p->view);
    } else if (!strcmp(cmd, "focus")) {
        if (number(o, "value") != 0)
            wpe_view_focus_in(p->view);
        else
            wpe_view_focus_out(p->view);
    } else if (!strcmp(cmd, "active")) {
        WPEToplevel *toplevel = wpe_view_get_toplevel(p->view);
        if (toplevel)
            wpe_toplevel_state_changed(toplevel, number(o, "value") != 0 ? WPE_TOPLEVEL_STATE_ACTIVE
                                                                         : WPE_TOPLEVEL_STATE_NONE);
    } else if (!strcmp(cmd, "eval"))
        webkit_web_view_evaluate_javascript(p->web, string(o, "script"), -1, NULL, NULL, NULL,
                                            evaluated, GUINT_TO_POINTER(id));
    else if (!strcmp(cmd, "scroll"))
        scroll_event(p, o);
    else if (!strcmp(cmd, "key_down") || !strcmp(cmd, "key_up"))
        key_event(p, o, !strcmp(cmd, "key_down"));
    else if (!strcmp(cmd, "move") || !strcmp(cmd, "down") || !strcmp(cmd, "up") ||
             !strcmp(cmd, "enter") || !strcmp(cmd, "leave"))
        pointer_event(p, o, cmd);
}
static gboolean read_commands(gint fd, GIOCondition condition, gpointer unused) {
    guint8 bytes[65536];
    ssize_t n = recv(fd, bytes, sizeof bytes, 0);
    if (n < 0 && errno == EINTR)
        return G_SOURCE_CONTINUE;
    if (n <= 0) {
        begin_shutdown();
        return G_SOURCE_REMOVE;
    }
    g_byte_array_append(input, bytes, n);
    while (input->len >= 4) {
        guint32 length;
        memcpy(&length, input->data, 4);
        length = GUINT32_FROM_LE(length);
        if (length > MAX_PACKET) {
            g_printerr("zeron-wpe: command of %u bytes exceeds the protocol limit\n", length);
            g_main_loop_quit(loop);
            return G_SOURCE_REMOVE;
        }
        if (input->len < 4 + length)
            break;
        JsonParser *parser = json_parser_new();
        if (json_parser_load_from_data(parser, (char *)input->data + 4, length, NULL)) {
            JsonNode *root = json_parser_get_root(parser);
            if (JSON_NODE_HOLDS_OBJECT(root))
                command(json_node_get_object(root));
        }
        g_object_unref(parser);
        g_byte_array_remove_range(input, 0, length + 4);
    }
    return G_SOURCE_CONTINUE;
}

// ---------------------------------------------------------------- startup
static gboolean storage_directory(const gchar *path) {
    if (g_mkdir_with_parents(path, 0700) < 0)
        return FALSE;
    gchar *probe = g_build_filename(path, ".write-test-XXXXXX", NULL);
    int fd = g_mkstemp(probe);
    if (fd >= 0) {
        close(fd);
        unlink(probe);
    }
    g_free(probe);
    return fd >= 0;
}
static void startup_error(const char *error) { send_packet('E', 0, error, strlen(error)); }
// Shares the WebKitGTK helper's profile layout, so either engine opens the
// same cookies and website data.
static gboolean configure_storage(const char *profile) {
    if (!profile) {
        session = webkit_network_session_new_ephemeral();
        return TRUE;
    }
    if (!g_path_is_absolute(profile)) {
        startup_error("Browser profile requires an absolute storage directory");
        return FALSE;
    }
    if (!storage_directory(profile)) {
        startup_error("Could not create or write the browser profile directory");
        return FALSE;
    }
    gchar *lock_path = g_build_filename(profile, "profile.lock", NULL);
    profile_lock = open(lock_path, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
    g_free(lock_path);
    if (profile_lock < 0 || flock(profile_lock, LOCK_EX | LOCK_NB) < 0) {
        startup_error("Browser profile is in use by another process or cannot be locked. Close "
                      "the other Zeron instance and reopen this tab.");
        return FALSE;
    }
    gchar *data = g_build_filename(profile, "data", NULL);
    gchar *cache = g_build_filename(profile, "cache", NULL);
    gchar *cookies = g_build_filename(profile, "cookies.sqlite", NULL);
    gboolean writable = storage_directory(data) && storage_directory(cache);
    int fd = writable ? open(cookies, O_RDWR | O_CREAT | O_CLOEXEC, 0600) : -1;
    if (fd < 0) {
        gchar *error = g_strdup_printf("Could not open browser profile: %s", g_strerror(errno));
        startup_error(error);
        g_free(error);
    } else {
        close(fd);
        session = webkit_network_session_new(data, cache);
        webkit_cookie_manager_set_persistent_storage(
            webkit_network_session_get_cookie_manager(session), cookies,
            WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE);
    }
    g_free(data);
    g_free(cache);
    g_free(cookies);
    return session != NULL;
}
static void configure_settings(void) {
    settings = webkit_settings_new();
    webkit_settings_set_enable_developer_extras(settings, FALSE);
    // WebKit otherwise rounds rendering updates to ~60 Hz on any display.
    WebKitFeatureList *features = webkit_settings_get_all_features();
    for (gsize i = 0; i < webkit_feature_list_get_length(features); i++) {
        WebKitFeature *feature = webkit_feature_list_get(features, i);
        if (!strcmp(webkit_feature_get_identifier(feature), "PreferPageRenderingUpdatesNear60FPS"))
            webkit_settings_set_feature_enabled(settings, feature, FALSE);
    }
    webkit_feature_list_unref(features);
}
// Keep the protocol socket away from stray stdout writes and child processes.
static gboolean take_socket(void) {
    int type = 0;
    socklen_t length = sizeof type;
    if (getsockopt(STDIN_FILENO, SOL_SOCKET, SO_TYPE, &type, &length) < 0 || type != SOCK_STREAM)
        return FALSE;
    proto = fcntl(STDIN_FILENO, F_DUPFD_CLOEXEC, 3);
    int null = open("/dev/null", O_RDONLY | O_CLOEXEC);
    if (proto < 0 || null < 0)
        return FALSE;
    dup2(null, STDIN_FILENO);
    dup2(STDERR_FILENO, STDOUT_FILENO);
    close(null);
    return TRUE;
}

// A comma-separated list of hexadecimal modifiers.
static GArray *parse_modifiers(const char *list) {
    GArray *modifiers = g_array_new(FALSE, FALSE, sizeof(guint64));
    gchar **items = g_strsplit(list, ",", -1);
    for (gchar **item = items; *item; item++) {
        guint64 modifier;
        if (g_ascii_string_to_unsigned(*item, 16, 0, G_MAXUINT64, &modifier, NULL))
            g_array_append_val(modifiers, modifier);
    }
    g_strfreev(items);
    return modifiers;
}

int main(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    umask(0077);
    const char *profile = NULL;
    for (int i = 1; i + 1 < argc; i += 2) {
        if (!strcmp(argv[i], "--profile"))
            profile = argv[i + 1];
        else if (!strcmp(argv[i], "--render-node"))
            render_node = argv[i + 1];
        else if (!strcmp(argv[i], "--modifiers"))
            importable = parse_modifiers(argv[i + 1]);
    }
    if (!take_socket()) {
        g_printerr("zeron-wpe: standard input must be the Zeron protocol socket\n");
        return 1;
    }
    loop = g_main_loop_new(NULL, FALSE);
    pages = g_hash_table_new_full(g_direct_hash, g_direct_equal, NULL, free_page);
    input = g_byte_array_new();
    display = g_object_new(z_display_get_type(), NULL);
    GError *error = NULL;
    if (!wpe_display_connect(display, &error)) {
        startup_error(error->message);
        return 1;
    }
    wpe_display_set_primary(display);
    probe_gpu((ZDisplay *)display);
    if (!configure_storage(profile))
        return 1;
    configure_settings();
    ticks = g_async_queue_new();
    GThread *vblank = g_thread_new("zeron-vblank", vblank_thread, NULL);
    JsonBuilder *b = json_builder_new();
    json_builder_begin_object(b);
    ZDisplay *d = (ZDisplay *)display;
    member_string(b, "gpu", d->drm ? render_node : NULL);
    member_string(b, "frames", !d->drm ? "shm" : d->shared ? "dmabuf" : "readback");
    json_builder_set_member_name(b, "modifiers");
    json_builder_begin_array(b);
    for (guint i = 0; d->drm && i < d->modifiers->len; i++)
        json_builder_add_int_value(b, (gint64)g_array_index(d->modifiers, guint64, i));
    json_builder_end_array(b);
    gchar *version = g_strdup_printf("%u.%u.%u", webkit_get_major_version(),
                                     webkit_get_minor_version(), webkit_get_micro_version());
    member_string(b, "webkit", version);
    g_free(version);
    json_builder_end_object(b);
    send_json('R', 0, b);
    g_signal_connect(session, "download-started", G_CALLBACK(download), NULL);
    g_unix_fd_add(proto, G_IO_IN | G_IO_HUP | G_IO_ERR, read_commands, NULL);
    g_main_loop_run(loop);
    shutting_down = TRUE;
    g_hash_table_destroy(pages);
    g_async_queue_push(ticks, &ticks);
    g_thread_join(vblank);
    g_byte_array_unref(input);
    g_object_unref(session);
    if (profile_lock >= 0)
        close(profile_lock);
    return 0;
}
