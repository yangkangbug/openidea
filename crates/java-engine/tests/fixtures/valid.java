// Written for java-engine. Not copied from IntelliJ test data.
package demo;

import java.util.List;
import static java.util.Collections.emptyList;

public class Hello {
    private final List<List<String>> names = List.of("你好");

    public Hello() {
        this.names.add("π");
    }

    public String greet(String name) {
        String message = "你好, " + name;
        if (name.length() > 0) {
            return message;
        } else {
            return "";
        }
    }
}
