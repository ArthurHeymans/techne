;;; Run a command in a headless Wayland session (sway), never the
;;; desktop's:
;;;
;;;   target/debug/techne run crates/techne-window/headless.scm COMMAND...
;;;
;;; Inside, WAYLAND_DISPLAY is the headless sway and DISPLAY is unset, so
;;; a window cannot land on the real desktop. `grim FILE.png` takes a
;;; screenshot and `wtype` types. Each wtype is a new virtual keyboard,
;;; and the keys it sends before the window has its keymap are lost: one,
;;; or several while a debug build is busy drawing. So start each with a
;;; key and a pause, `wtype -k Shift_L -s 300 ...`, and check what
;;; arrived by saving the file, not only by looking. With a GPU render
;;; node, sway composites the client's GPU buffers; without one it falls
;;; back to software rendering. Needs `nix develop .#window`. It exits as
;;; COMMAND does.

(import (techne) (techne crypto) (srfi 115))

(define command (cdr (command-line)))
(when (null? command)
  (display "usage: techne run headless.scm COMMAND...\n" (current-error-port))
  (exit 2))

;; A render node Mesa drives: sway is pointed at Mesa's EGL below, and on
;; NVIDIA's node the client's EGL initialization fails (wgpu then
;; crashes in its EGL debug callback).
(define render
  (let ((nodes (guard (e (#t '()))
                 (filter (lambda (n) (string-prefix? "renderD" n)) (directory-list "/dev/dri")))))
    (find (lambda (node)
            (let ((driver (guard (e (#t ""))
                            (file-truename (string-append "/sys/class/drm/" node "/device/driver")))))
              (not (regexp-search '(: "/nvidia" eos) driver))))
          nodes)))

;; On NixOS with NVIDIA's EGL installed next to Mesa's, sway must use
;; Mesa's for the render node.
(define mesa-egl "/run/opengl-driver/share/glvnd/egl_vendor.d/50_mesa.json")
(define egl (if (file-exists? mesa-egl) `(("__EGL_VENDOR_LIBRARY_FILENAMES" . ,mesa-egl)) '()))

;; Nothing from the desktop's session.
(define desktop '(("WAYLAND_DISPLAY" . #f) ("DISPLAY" . #f) ("SWAYSOCK" . #f)))

;; Sway in DIR, until COMMAND ends: its status. Leaving, however it
;; leaves (an error, or this interrupted by Ctrl-C, SIGTERM or SIGHUP),
;; kills COMMAND's processes and sway's.
(define (session)
  (call-with-output-file (in-dir "sway.conf")
    (lambda (p)
      (display "xwayland disable\noutput HEADLESS-1 resolution 1600x1000 scale 1\ndefault_border none\n" p)))
  (call-with-process "sway" (list "-c" (in-dir "sway.conf"))
    (lambda (sway)
      (let wait ((tries 100))
        (unless (file-exists? (in-dir "wayland-1"))
          (when (or (zero? tries) (process-exited? sway))
            (for-each (lambda (log) (display (file->string (in-dir log)) (current-error-port)))
                      '("sway.log" "sway.err"))
            (error "headless: sway did not start"))
          (sleep 50)
          (wait (- tries 1))))
      (call-with-process (car command) (cdr command) process-wait
        #:stdin 'inherit #:stdout 'inherit #:stderr 'inherit
        #:environment (append desktop egl
                              `(("XDG_RUNTIME_DIR" . ,dir) ("WAYLAND_DISPLAY" . "wayland-1")))))
    #:stdin 'null #:stdout (in-dir "sway.log") #:stderr (in-dir "sway.err")
    #:environment (append desktop egl
                          `(("XDG_RUNTIME_DIR" . ,dir) ("WLR_BACKENDS" . "headless")
                            ("WLR_LIBINPUT_NO_DEVICES" . "1"))
                          (if render
                              `(("WLR_RENDER_DRM_DEVICE" . ,(string-append "/dev/dri/" render)))
                              '()))))

;; A directory of its own for sway's socket, its owner's only, as
;; XDG_RUNTIME_DIR must be.
(define dir
  (let make ((tries 16))
    (let ((path (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                               "/techne-headless." (hex-encode (random-bytes 6)))))
      (cond ((guard (e ((zero? tries) (raise e)) (#t #f)) (create-directory path #o700) #t) path)
            (else (make (- tries 1)))))))
(define (in-dir name) (string-append dir "/" name))

;; The directory goes whatever happens once it is made.
(define status
  (dynamic-wind (lambda () #f) session (lambda () (delete-path dir #t))))
;; A command killed by a signal exits as a shell says so.
(exit (if (pair? status) (+ 128 (cadr status)) status))
