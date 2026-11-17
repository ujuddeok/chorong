public class Lambdas {
    interface F<A, B> { B apply(A a); }
    static <A, B, C> F<A, C> compose(F<A, B> f, F<B, C> g) { return a -> g.apply(f.apply(a)); }
    Object o = new Object() { int hashCode = 1; class Inner { void x() {} } };
    Runnable[] rs = { () -> {}, new Runnable() { public void run() { new Thread(() -> { new Object() { void deep() {} }; }).start(); } } };
    @FunctionalInterface interface G { void go(); }
    static int[][] grid = new int[3][];
    void varargs(String... xs) { java.util.Arrays.asList(xs).stream().map(String::length).filter(n -> n > 0).count(); }
    <T extends Number & Comparable<T>> T max(T a, T b) { return a.compareTo(b) > 0 ? a : b; }
    enum Empty {}
    enum Single { ONLY; }
}
