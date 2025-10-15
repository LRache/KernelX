function(parse_config config_file)
    file(STRINGS "${config_file}" lines)

    foreach(line IN LISTS lines)
        if(line MATCHES "^$" OR line MATCHES "^#")
            continue()
        endif()
        
        if(line MATCHES "^([A-Za-z0-9_]+)=([yYnN]|\".*\"|[0-9]+|[^\"\n]+)$")
            set(_var_name "${CMAKE_MATCH_1}")
            set(_var_value "${CMAKE_MATCH_2}")

            if(_var_value MATCHES "^(y|Y)$")
                set(_var_value ON)
            elseif(_var_value MATCHES "^(n|N)$")
                set(_var_value OFF)
            elseif(_var_value MATCHES "^\".*\"$")
                string(REGEX REPLACE "^\"(.*)\"$" "\\1" _var_value "${_var_value}")
            endif()

            set(${_var_name} "${_var_value}" PARENT_SCOPE)
        endif()
    endforeach()
endfunction()