// The C the Rill program is a translation of: same window, same shaders, same
// vertex data, same seven-segment frame counter, same twenty-second run. Kept
// next to it so the two can be read side by side.
//
//   cc -O2 -o triangle triangle.c -I/opt/homebrew/include -L/opt/homebrew/lib \
//      -lglfw -framework OpenGL
//
// Written the way a C programmer would write it — loops where Rill recurses,
// mutable state where Rill threads values through — rather than as a
// transliteration.

#define GL_SILENCE_DEPRECATION
#define GLFW_INCLUDE_GLCOREARB
#include <GLFW/glfw3.h>

#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#define RUN_SECONDS 20.0

static const char *VERTEX_SRC =
    "#version 150 core\n"
    "in vec2 pos;\n"
    "in vec3 tint;\n"
    "out vec3 shade;\n"
    "uniform float angle;\n"
    "void main() {\n"
    "  float c = cos(angle);\n"
    "  float s = sin(angle);\n"
    "  gl_Position = vec4(pos.x * c - pos.y * s, pos.x * s + pos.y * c, 0.0, 1.0);\n"
    "  shade = tint;\n"
    "}\n";

static const char *FRAGMENT_SRC =
    "#version 150 core\n"
    "in vec3 shade;\n"
    "out vec4 color;\n"
    "void main() {\n"
    "  color = vec4(shade, 1.0);\n"
    "}\n";

static const char *HUD_VERTEX_SRC =
    "#version 150 core\n"
    "in vec2 corner;\n"
    "uniform vec4 rect;\n"
    "void main() {\n"
    "  gl_Position = vec4(rect.xy + corner * rect.zw, 0.0, 1.0);\n"
    "}\n";

static const char *HUD_FRAGMENT_SRC =
    "#version 150 core\n"
    "uniform vec3 ink;\n"
    "out vec4 color;\n"
    "void main() {\n"
    "  color = vec4(ink, 1.0);\n"
    "}\n";

// Which of the seven segments each digit lights: 1 top, 2 top right,
// 4 bottom right, 8 bottom, 16 bottom left, 32 top left, 64 middle.
static const int SEGMENTS[10] = {63, 6, 91, 79, 102, 109, 125, 7, 127, 111};

static void die(const char *what, const char *detail) {
    fprintf(stderr, "%s failed: %s\n", what, detail);
    exit(1);
}

static void on_error(int code, const char *description) {
    printf("glfw error %d: %s\n", code, description);
}

static void on_key(GLFWwindow *win, int key, int scancode, int action, int mods) {
    (void)scancode;
    (void)mods;
    if (key == GLFW_KEY_ESCAPE && action == GLFW_PRESS)
        glfwSetWindowShouldClose(win, 1);
}

static void on_resize(GLFWwindow *win, int w, int h) {
    (void)win;
    glViewport(0, 0, w, h);
}

static GLuint compile_shader(GLenum kind, const char *src) {
    GLuint sh = glCreateShader(kind);
    glShaderSource(sh, 1, &src, NULL);
    glCompileShader(sh);
    GLint ok = 0;
    glGetShaderiv(sh, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[1024];
        glGetShaderInfoLog(sh, sizeof log, NULL, log);
        die("shader", log);
    }
    return sh;
}

static GLuint link_program(const char *vs_src, const char *fs_src,
                           const char *const *attribs, int nattribs) {
    GLuint prog = glCreateProgram();
    glAttachShader(prog, compile_shader(GL_VERTEX_SHADER, vs_src));
    glAttachShader(prog, compile_shader(GL_FRAGMENT_SHADER, fs_src));
    for (int i = 0; i < nattribs; i++)
        glBindAttribLocation(prog, i, attribs[i]);
    glLinkProgram(prog);
    GLint ok = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &ok);
    if (!ok) {
        char log[1024];
        glGetProgramInfoLog(prog, sizeof log, NULL, log);
        die("link", log);
    }
    return prog;
}

// Five floats per vertex: a position and a colour.
static GLuint upload_triangle(void) {
    static const float DATA[] = {
        0.00f,  0.62f,  0.99f, 0.75f, 0.18f,
        -0.55f, -0.36f, 0.13f, 0.72f, 0.85f,
        0.55f,  -0.36f, 0.85f, 0.22f, 0.45f,
    };
    GLuint vao, vbo;
    glGenVertexArrays(1, &vao);
    glBindVertexArray(vao);
    glGenBuffers(1, &vbo);
    glBindBuffer(GL_ARRAY_BUFFER, vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof DATA, DATA, GL_STATIC_DRAW);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 20, (void *)0);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(1, 3, GL_FLOAT, GL_FALSE, 20, (void *)8);
    glEnableVertexAttribArray(1);
    return vao;
}

// The quad every segment is drawn from: two triangles over the unit square.
static GLuint hud_quad(void) {
    static const float DATA[] = {
        0.0f, 0.0f, 1.0f, 0.0f, 1.0f, 1.0f,
        0.0f, 0.0f, 1.0f, 1.0f, 0.0f, 1.0f,
    };
    GLuint vao, vbo;
    glGenVertexArrays(1, &vao);
    glBindVertexArray(vao);
    glGenBuffers(1, &vbo);
    glBindBuffer(GL_ARRAY_BUFFER, vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof DATA, DATA, GL_STATIC_DRAW);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 8, (void *)0);
    glEnableVertexAttribArray(0);
    return vao;
}

// A digit cell is w by h with its lower left corner at (x, y); t is the
// stroke. Each segment is one rectangle in those terms.
static void draw_digit(GLint rect_at, int digit, float x, float y, float w,
                       float h, float t) {
    const float boxes[7][4] = {
        {x, y + h - t, w, t},                  // top
        {x + w - t, y + h / 2, t, h / 2},      // top right
        {x + w - t, y, t, h / 2},              // bottom right
        {x, y, w, t},                          // bottom
        {x, y, t, h / 2},                      // bottom left
        {x, y + h / 2, t, h / 2},              // top left
        {x, y + h / 2 - t / 2, w, t},          // middle
    };
    int mask = SEGMENTS[digit];
    for (int s = 0; s < 7; s++) {
        if (!(mask >> s & 1))
            continue;
        glUniform4f(rect_at, boxes[s][0], boxes[s][1], boxes[s][2], boxes[s][3]);
        glDrawArrays(GL_TRIANGLES, 0, 6);
    }
}

// Digits are drawn from the right, so the number grows leftwards.
static void draw_fps(GLuint prog, GLuint vao, GLint rect_at, GLint ink_at, int fps) {
    const float w = 0.048f, h = 0.10f, t = 0.012f;
    glUseProgram(prog);
    glBindVertexArray(vao);
    glUniform3f(ink_at, 0.55f, 0.62f, 0.70f);
    float x = -0.80f;
    if (fps < 0)
        fps = 0;
    do {
        draw_digit(rect_at, fps % 10, x, 0.82f, w, h, t);
        x -= w + t * 2;
        fps /= 10;
    } while (fps > 0);
}

int main(void) {
    glfwSetErrorCallback(on_error);
    if (!glfwInit())
        die("glfwInit", "no display");

    glfwWindowHint(GLFW_CONTEXT_VERSION_MAJOR, 3);
    glfwWindowHint(GLFW_CONTEXT_VERSION_MINOR, 2);
    glfwWindowHint(GLFW_OPENGL_PROFILE, GLFW_OPENGL_CORE_PROFILE);
    glfwWindowHint(GLFW_OPENGL_FORWARD_COMPAT, GLFW_TRUE);
    GLFWwindow *win = glfwCreateWindow(640, 480, "C · OpenGL", NULL, NULL);
    if (!win)
        die("window", "no OpenGL 3.2 context");

    glfwMakeContextCurrent(win);
    glfwSwapInterval(1);
    glfwSetKeyCallback(win, on_key);
    glfwSetFramebufferSizeCallback(win, on_resize);
    int fw, fh;
    glfwGetFramebufferSize(win, &fw, &fh);
    glViewport(0, 0, fw, fh);
    printf("GL %s\n", glGetString(GL_VERSION));

    static const char *const SCENE_ATTRIBS[] = {"pos", "tint"};
    static const char *const HUD_ATTRIBS[] = {"corner"};
    GLuint prog = link_program(VERTEX_SRC, FRAGMENT_SRC, SCENE_ATTRIBS, 2);
    GLuint vao = upload_triangle();
    GLint angle_at = glGetUniformLocation(prog, "angle");
    GLuint hud_prog = link_program(HUD_VERTEX_SRC, HUD_FRAGMENT_SRC, HUD_ATTRIBS, 1);
    GLuint hud_vao = hud_quad();
    GLint rect_at = glGetUniformLocation(hud_prog, "rect");
    GLint ink_at = glGetUniformLocation(hud_prog, "ink");

    double until = glfwGetTime() + RUN_SECONDS, since = glfwGetTime();
    int frames = 0, fps = 0;

    while (!glfwWindowShouldClose(win) && glfwGetTime() <= until) {
        glClearColor(0.05f, 0.06f, 0.09f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);
        glUseProgram(prog);
        glUniform1f(angle_at, (float)glfwGetTime());
        glBindVertexArray(vao);
        glDrawArrays(GL_TRIANGLES, 0, 3);
        draw_fps(hud_prog, hud_vao, rect_at, ink_at, fps);
        glfwSwapBuffers(win);
        glfwPollEvents();

        // Counting over a whole second is what makes the reading a rate
        // without ever dividing.
        frames++;
        if (glfwGetTime() - since >= 1.0) {
            fps = frames;
            frames = 0;
            since = glfwGetTime();
        }
    }

    glfwTerminate();
    return 0;
}
